// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {MeshEscrow} from "../src/MeshEscrow.sol";
import {ERC20} from "openzeppelin/token/ERC20/ERC20.sol";

contract MockUSDC is ERC20 {
    constructor() ERC20("Mock USDC", "mUSDC") {}

    function mint(address to, uint256 amount) external {
        _mint(to, amount);
    }

    function decimals() public pure override returns (uint8) {
        return 6;
    }
}

contract MeshEscrowTest is Test {
    MeshEscrow internal escrow;
    MockUSDC internal usdc;

    uint256 internal payerKey = 0xA11CE;
    uint256 internal recipientKey = 0xB0B;
    address internal payer;
    address internal recipient;

    bytes32 internal constant RECEIPT_TYPEHASH =
        keccak256("Receipt(bytes32 sessionId,uint256 cumulative)");

    function setUp() public {
        escrow = new MeshEscrow();
        usdc = new MockUSDC();
        payer = vm.addr(payerKey);
        recipient = vm.addr(recipientKey);

        vm.deal(payer, 100 ether);
        usdc.mint(payer, 1_000_000e6);
        vm.prank(payer);
        usdc.approve(address(escrow), type(uint256).max);
    }

    function _signReceipt(bytes32 sessionId, uint256 cumulative, uint256 key)
        internal
        view
        returns (bytes memory)
    {
        bytes32 digest = escrow.receiptDigest(sessionId, cumulative);
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(key, digest);
        return abi.encodePacked(r, s, v);
    }

    // ---------- Open ----------

    function test_open_eth() public {
        bytes32 sid = bytes32(uint256(1));
        vm.prank(payer);
        escrow.open{value: 1 ether}(sid, recipient, address(0), 1 ether, uint64(block.timestamp + 1 days));
        (address p,,,uint256 dep,,) = escrow.getChannel(sid);
        assertEq(p, payer);
        assertEq(dep, 1 ether);
    }

    function test_open_usdc() public {
        bytes32 sid = bytes32(uint256(2));
        vm.prank(payer);
        escrow.open(sid, recipient, address(usdc), 100e6, uint64(block.timestamp + 1 days));
        assertEq(usdc.balanceOf(address(escrow)), 100e6);
    }

    function test_open_revert_eth_with_token() public {
        bytes32 sid = bytes32(uint256(3));
        vm.prank(payer);
        vm.expectRevert(MeshEscrow.WrongValue.selector);
        escrow.open{value: 1 ether}(sid, recipient, address(usdc), 100e6, uint64(block.timestamp + 1 days));
    }

    function test_open_revert_eth_value_mismatch() public {
        bytes32 sid = bytes32(uint256(4));
        vm.prank(payer);
        vm.expectRevert(MeshEscrow.WrongValue.selector);
        escrow.open{value: 0.5 ether}(sid, recipient, address(0), 1 ether, uint64(block.timestamp + 1 days));
    }

    function test_open_revert_zero_deposit() public {
        bytes32 sid = bytes32(uint256(5));
        vm.prank(payer);
        vm.expectRevert(MeshEscrow.ZeroDeposit.selector);
        escrow.open{value: 0}(sid, recipient, address(0), 0, uint64(block.timestamp + 1 days));
    }

    function test_open_revert_past_deadline() public {
        bytes32 sid = bytes32(uint256(6));
        vm.prank(payer);
        vm.expectRevert(MeshEscrow.PastDeadline.selector);
        escrow.open{value: 1 ether}(sid, recipient, address(0), 1 ether, uint64(block.timestamp));
    }

    function test_open_revert_duplicate() public {
        bytes32 sid = bytes32(uint256(7));
        vm.startPrank(payer);
        escrow.open{value: 1 ether}(sid, recipient, address(0), 1 ether, uint64(block.timestamp + 1 days));
        vm.expectRevert(MeshEscrow.ChannelExists.selector);
        escrow.open{value: 1 ether}(sid, recipient, address(0), 1 ether, uint64(block.timestamp + 1 days));
        vm.stopPrank();
    }

    // ---------- Claim ----------

    function test_claim_eth_full() public {
        bytes32 sid = bytes32(uint256(10));
        vm.prank(payer);
        escrow.open{value: 1 ether}(sid, recipient, address(0), 1 ether, uint64(block.timestamp + 1 days));

        bytes memory sig = _signReceipt(sid, 1 ether, payerKey);
        uint256 before = recipient.balance;
        vm.prank(recipient);
        escrow.claim(sid, 1 ether, sig);
        assertEq(recipient.balance - before, 1 ether);
    }

    /// Defense-in-depth: a third party with a copy of a valid receipt cannot
    /// trigger the payout — that would otherwise let an attacker grief a
    /// recipient contract that reverts on receive.
    function test_claim_only_recipient_can_call() public {
        bytes32 sid = bytes32(uint256(99));
        vm.prank(payer);
        escrow.open{value: 1 ether}(sid, recipient, address(0), 1 ether, uint64(block.timestamp + 1 days));
        bytes memory sig = _signReceipt(sid, 1 ether, payerKey);
        address third = address(0xdeadbeef);
        vm.prank(third);
        vm.expectRevert(MeshEscrow.NotRecipient.selector);
        escrow.claim(sid, 1 ether, sig);
    }

    function test_claim_usdc_partial_then_partial() public {
        bytes32 sid = bytes32(uint256(11));
        vm.prank(payer);
        escrow.open(sid, recipient, address(usdc), 100e6, uint64(block.timestamp + 1 days));

        // Pre-compute sigs: _signReceipt calls escrow.receiptDigest which is
        // an external view call — running it inside the claim() argument list
        // consumes the vm.prank(recipient) before claim() executes.
        bytes memory sig1 = _signReceipt(sid, 30e6, payerKey);
        vm.prank(recipient);
        escrow.claim(sid, 30e6, sig1);
        assertEq(usdc.balanceOf(recipient), 30e6);

        bytes memory sig2 = _signReceipt(sid, 75e6, payerKey);
        vm.prank(recipient);
        escrow.claim(sid, 75e6, sig2);
        assertEq(usdc.balanceOf(recipient), 75e6);
    }

    function test_claim_revert_decreasing() public {
        bytes32 sid = bytes32(uint256(12));
        vm.prank(payer);
        escrow.open{value: 1 ether}(sid, recipient, address(0), 1 ether, uint64(block.timestamp + 1 days));

        bytes memory firstSig = _signReceipt(sid, 0.5 ether, payerKey);
        vm.prank(recipient);
        escrow.claim(sid, 0.5 ether, firstSig);

        // Pre-compute the signature so the receiptDigest() view call doesn't
        // consume vm.expectRevert before the actual claim() runs.
        bytes memory secondSig = _signReceipt(sid, 0.3 ether, payerKey);
        vm.prank(recipient);
        vm.expectRevert(MeshEscrow.AmountNotIncreasing.selector);
        escrow.claim(sid, 0.3 ether, secondSig);
    }

    function test_claim_revert_exceeds_deposit() public {
        bytes32 sid = bytes32(uint256(13));
        vm.prank(payer);
        escrow.open{value: 1 ether}(sid, recipient, address(0), 1 ether, uint64(block.timestamp + 1 days));

        bytes memory sig = _signReceipt(sid, 2 ether, payerKey);
        vm.prank(recipient);
        vm.expectRevert(MeshEscrow.ExceedsDeposit.selector);
        escrow.claim(sid, 2 ether, sig);
    }

    function test_claim_revert_wrong_signer() public {
        bytes32 sid = bytes32(uint256(14));
        vm.prank(payer);
        escrow.open{value: 1 ether}(sid, recipient, address(0), 1 ether, uint64(block.timestamp + 1 days));

        // Recipient signs their own receipt (forgery attempt).
        bytes memory sig = _signReceipt(sid, 1 ether, recipientKey);
        vm.prank(recipient);
        vm.expectRevert(MeshEscrow.BadSignature.selector);
        escrow.claim(sid, 1 ether, sig);
    }

    function test_claim_revert_missing_channel() public {
        bytes32 sid = bytes32(uint256(15));
        bytes memory sig = _signReceipt(sid, 1 ether, payerKey);
        vm.prank(recipient);
        vm.expectRevert(MeshEscrow.ChannelMissing.selector);
        escrow.claim(sid, 1 ether, sig);
    }

    // ---------- Close ----------

    function test_close_refunds_unspent() public {
        bytes32 sid = bytes32(uint256(20));
        uint64 deadline = uint64(block.timestamp + 1 days);
        vm.prank(payer);
        escrow.open{value: 1 ether}(sid, recipient, address(0), 1 ether, deadline);

        // Recipient claims 0.4 (pre-compute sig — see test_claim_usdc note)
        bytes memory sig = _signReceipt(sid, 0.4 ether, payerKey);
        vm.prank(recipient);
        escrow.claim(sid, 0.4 ether, sig);

        // Warp past deadline, payer closes, gets remaining 0.6 back.
        vm.warp(deadline + 1);
        uint256 before = payer.balance;
        vm.prank(payer);
        escrow.close(sid);
        assertEq(payer.balance - before, 0.6 ether);
        (address p,,,,, ) = escrow.getChannel(sid);
        assertEq(p, address(0)); // channel deleted
    }

    function test_close_revert_not_expired() public {
        bytes32 sid = bytes32(uint256(21));
        vm.prank(payer);
        escrow.open{value: 1 ether}(sid, recipient, address(0), 1 ether, uint64(block.timestamp + 1 days));
        vm.prank(payer);
        vm.expectRevert(MeshEscrow.NotExpired.selector);
        escrow.close(sid);
    }

    function test_close_revert_not_payer() public {
        bytes32 sid = bytes32(uint256(23));
        uint64 deadline = uint64(block.timestamp + 1 days);
        vm.prank(payer);
        escrow.open{value: 1 ether}(sid, recipient, address(0), 1 ether, deadline);
        vm.warp(deadline + 1);
        vm.prank(recipient);
        vm.expectRevert(MeshEscrow.NotPayer.selector);
        escrow.close(sid);
    }

    function test_claim_revert_not_recipient() public {
        bytes32 sid = bytes32(uint256(24));
        vm.prank(payer);
        escrow.open{value: 1 ether}(sid, recipient, address(0), 1 ether, uint64(block.timestamp + 1 days));
        bytes memory sig = _signReceipt(sid, 1 ether, payerKey);
        // Third party (payer themselves here, just as a non-recipient) cannot
        // trigger the payout — would otherwise be a griefing vector against
        // a recipient contract that reverts on receive.
        vm.prank(payer);
        vm.expectRevert(MeshEscrow.NotRecipient.selector);
        escrow.claim(sid, 1 ether, sig);
    }

    function test_close_no_claim_full_refund() public {
        bytes32 sid = bytes32(uint256(22));
        uint64 deadline = uint64(block.timestamp + 1 days);
        vm.prank(payer);
        escrow.open{value: 1 ether}(sid, recipient, address(0), 1 ether, deadline);

        vm.warp(deadline + 1);
        uint256 before = payer.balance;
        vm.prank(payer);
        escrow.close(sid);
        assertEq(payer.balance - before, 1 ether);
    }

    // ---------- Batched claim ----------

    function test_claim_batch_settles_n_in_one_tx() public {
        // Open 3 ETH channels, all funded by `payer`, all to `recipient`.
        bytes32 sidA = bytes32(uint256(40));
        bytes32 sidB = bytes32(uint256(41));
        bytes32 sidC = bytes32(uint256(42));
        uint64 deadline = uint64(block.timestamp + 1 days);
        vm.startPrank(payer);
        escrow.open{value: 1 ether}(sidA, recipient, address(0), 1 ether, deadline);
        escrow.open{value: 1 ether}(sidB, recipient, address(0), 1 ether, deadline);
        escrow.open{value: 1 ether}(sidC, recipient, address(0), 1 ether, deadline);
        vm.stopPrank();

        bytes32[] memory ids = new bytes32[](3);
        uint256[] memory cums = new uint256[](3);
        bytes[] memory sigs = new bytes[](3);
        ids[0] = sidA; cums[0] = 0.3 ether;
        ids[1] = sidB; cums[1] = 0.4 ether;
        ids[2] = sidC; cums[2] = 0.5 ether;
        sigs[0] = _signReceipt(sidA, 0.3 ether, payerKey);
        sigs[1] = _signReceipt(sidB, 0.4 ether, payerKey);
        sigs[2] = _signReceipt(sidC, 0.5 ether, payerKey);

        uint256 before = recipient.balance;
        vm.prank(recipient);
        escrow.claimBatch(ids, cums, sigs);
        assertEq(recipient.balance - before, 1.2 ether);
    }

    function test_claim_batch_revert_on_bad_signature_rolls_back_all() public {
        bytes32 sidA = bytes32(uint256(50));
        bytes32 sidB = bytes32(uint256(51));
        uint64 deadline = uint64(block.timestamp + 1 days);
        vm.startPrank(payer);
        escrow.open{value: 1 ether}(sidA, recipient, address(0), 1 ether, deadline);
        escrow.open{value: 1 ether}(sidB, recipient, address(0), 1 ether, deadline);
        vm.stopPrank();

        bytes32[] memory ids = new bytes32[](2);
        uint256[] memory cums = new uint256[](2);
        bytes[] memory sigs = new bytes[](2);
        ids[0] = sidA; cums[0] = 0.3 ether;
        ids[1] = sidB; cums[1] = 0.4 ether;
        sigs[0] = _signReceipt(sidA, 0.3 ether, payerKey);
        // Second sig is forged with the recipient's key — must abort the whole batch.
        sigs[1] = _signReceipt(sidB, 0.4 ether, recipientKey);

        uint256 before = recipient.balance;
        vm.prank(recipient);
        vm.expectRevert(MeshEscrow.BadSignature.selector);
        escrow.claimBatch(ids, cums, sigs);
        assertEq(recipient.balance, before, "no partial payout on revert");
        // Channel A's `paid` must still be 0 — atomic batch, not best-effort.
        (,,,, uint256 paidA,) = escrow.getChannel(sidA);
        assertEq(paidA, 0);
    }

    function test_claim_batch_length_mismatch_reverts() public {
        bytes32[] memory ids = new bytes32[](2);
        uint256[] memory cums = new uint256[](1);
        bytes[] memory sigs = new bytes[](2);
        vm.prank(recipient);
        vm.expectRevert(MeshEscrow.LengthMismatch.selector);
        escrow.claimBatch(ids, cums, sigs);
    }

    // ---------- Replay safety: a signed receipt for one session cannot be
    // reused on a different session ----------

    function test_receipt_bound_to_session() public {
        bytes32 sidA = bytes32(uint256(30));
        bytes32 sidB = bytes32(uint256(31));
        vm.startPrank(payer);
        escrow.open{value: 1 ether}(sidA, recipient, address(0), 1 ether, uint64(block.timestamp + 1 days));
        escrow.open{value: 1 ether}(sidB, recipient, address(0), 1 ether, uint64(block.timestamp + 1 days));
        vm.stopPrank();

        // Sig for A presented against B should fail signature check.
        bytes memory sigForA = _signReceipt(sidA, 0.5 ether, payerKey);
        vm.prank(recipient);
        vm.expectRevert(MeshEscrow.BadSignature.selector);
        escrow.claim(sidB, 0.5 ether, sigForA);
    }
}
