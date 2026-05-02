// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {IERC20} from "openzeppelin/token/ERC20/IERC20.sol";
import {SafeERC20} from "openzeppelin/token/ERC20/utils/SafeERC20.sol";
import {ECDSA} from "openzeppelin/utils/cryptography/ECDSA.sol";
import {EIP712} from "openzeppelin/utils/cryptography/EIP712.sol";
import {ReentrancyGuard} from "openzeppelin/utils/ReentrancyGuard.sol";

/// @title MeshEscrow
/// @notice Per-session payment channels for tkr-mesh agents.
///
/// Flow:
///   1. Payer opens a channel: deposits `amount` of `token` for `recipient`,
///      tagged with a unique `sessionId` and an `expiresAt` deadline.
///   2. Recipient (off-chain) accumulates work and signs receipts from the
///      payer for cumulative amounts (each new receipt supersedes the prior).
///   3. Recipient calls `claim(sessionId, cumulative, sig)` — contract pays
///      out (cumulative - alreadyPaid) and updates state. Can be called
///      multiple times with monotonically increasing `cumulative`.
///   4. After `expiresAt`, payer can call `close(sessionId)` to reclaim any
///      unsettled funds.
///
/// Token model: `token == address(0)` means native ETH; any other address is
/// an ERC-20. USDC on Base is 6-decimals — callers convert dollar amounts.
contract MeshEscrow is EIP712, ReentrancyGuard {
    using SafeERC20 for IERC20;

    // ---------- Types ----------

    struct Channel {
        address payer;
        address recipient;
        address token; // address(0) = ETH
        uint256 deposit;
        uint256 paid; // cumulative amount already released to recipient
        uint64 expiresAt;
    }

    bytes32 private constant RECEIPT_TYPEHASH =
        keccak256("Receipt(bytes32 sessionId,uint256 cumulative)");

    // ---------- State ----------

    mapping(bytes32 => Channel) public channels;

    // ---------- Events ----------

    event ChannelOpened(
        bytes32 indexed sessionId,
        address indexed payer,
        address indexed recipient,
        address token,
        uint256 deposit,
        uint64 expiresAt
    );
    event Claimed(bytes32 indexed sessionId, uint256 cumulative, uint256 paidOut);
    event Closed(bytes32 indexed sessionId, uint256 refunded);

    // ---------- Errors ----------

    error ChannelExists();
    error ChannelMissing();
    error WrongToken();
    error WrongValue();
    error ZeroDeposit();
    error PastDeadline();
    error NotExpired();
    error NotPayer();
    error NotRecipient();
    error BadSignature();
    error AmountNotIncreasing();
    error ExceedsDeposit();

    constructor() EIP712("tkr-mesh", "1") {}

    // ---------- Open ----------

    /// @notice Open a new payment channel. Native-ETH variant requires
    ///         `msg.value == amount` and `token == address(0)`.
    function open(
        bytes32 sessionId,
        address recipient,
        address token,
        uint256 amount,
        uint64 expiresAt
    ) external payable nonReentrant {
        if (amount == 0) revert ZeroDeposit();
        if (expiresAt <= block.timestamp) revert PastDeadline();
        if (channels[sessionId].payer != address(0)) revert ChannelExists();

        if (token == address(0)) {
            if (msg.value != amount) revert WrongValue();
        } else {
            if (msg.value != 0) revert WrongValue();
            IERC20(token).safeTransferFrom(msg.sender, address(this), amount);
        }

        channels[sessionId] = Channel({
            payer: msg.sender,
            recipient: recipient,
            token: token,
            deposit: amount,
            paid: 0,
            expiresAt: expiresAt
        });

        emit ChannelOpened(sessionId, msg.sender, recipient, token, amount, expiresAt);
    }

    // ---------- Claim ----------

    /// @notice Recipient claims funds against a payer-signed receipt for
    ///         cumulative paid amount. Pays out (cumulative - prev.paid).
    function claim(bytes32 sessionId, uint256 cumulative, bytes calldata signature)
        external
        nonReentrant
    {
        Channel storage ch = channels[sessionId];
        if (ch.payer == address(0)) revert ChannelMissing();
        // Restrict to recipient: a third party with a copy of a valid receipt
        // could otherwise force a payout to a contract that reverts on
        // receive, locking the channel until expiry.
        if (msg.sender != ch.recipient) revert NotRecipient();
        if (cumulative <= ch.paid) revert AmountNotIncreasing();
        if (cumulative > ch.deposit) revert ExceedsDeposit();

        bytes32 digest = _hashTypedDataV4(
            keccak256(abi.encode(RECEIPT_TYPEHASH, sessionId, cumulative))
        );
        address signer = ECDSA.recover(digest, signature);
        if (signer != ch.payer) revert BadSignature();

        uint256 payout = cumulative - ch.paid;
        ch.paid = cumulative;

        _payOut(ch.token, ch.recipient, payout);
        emit Claimed(sessionId, cumulative, payout);
    }

    // ---------- Close ----------

    /// @notice Payer reclaims unsettled funds after the deadline. Restricted
    ///         to the payer: a permissionless `close()` lets a MEV bot front-
    ///         run a recipient's expiry-window claim and refund the payer for
    ///         work already performed.
    function close(bytes32 sessionId) external nonReentrant {
        Channel storage ch = channels[sessionId];
        if (ch.payer == address(0)) revert ChannelMissing();
        if (msg.sender != ch.payer) revert NotPayer();
        if (block.timestamp < ch.expiresAt) revert NotExpired();

        uint256 remaining = ch.deposit - ch.paid;
        address payer = ch.payer;
        address token = ch.token;
        delete channels[sessionId];

        if (remaining > 0) {
            _payOut(token, payer, remaining);
        }
        emit Closed(sessionId, remaining);
    }

    // ---------- Views ----------

    /// @notice Compute the EIP-712 digest a payer must sign to authorize
    ///         a `cumulative` payout for `sessionId`. Off-chain callers use
    ///         this to display the digest to the user before signing.
    function receiptDigest(bytes32 sessionId, uint256 cumulative) external view returns (bytes32) {
        return _hashTypedDataV4(keccak256(abi.encode(RECEIPT_TYPEHASH, sessionId, cumulative)));
    }

    /// @notice Convenience getter — Solidity's auto-generated channel()
    ///         requires returning the struct as a tuple, this is friendlier.
    function getChannel(bytes32 sessionId)
        external
        view
        returns (
            address payer,
            address recipient,
            address token,
            uint256 deposit,
            uint256 paid,
            uint64 expiresAt
        )
    {
        Channel memory ch = channels[sessionId];
        return (ch.payer, ch.recipient, ch.token, ch.deposit, ch.paid, ch.expiresAt);
    }

    // ---------- Internal ----------

    function _payOut(address token, address to, uint256 amount) private {
        if (amount == 0) return;
        if (token == address(0)) {
            (bool ok,) = payable(to).call{value: amount}("");
            require(ok, "eth transfer failed");
        } else {
            IERC20(token).safeTransfer(to, amount);
        }
    }
}
