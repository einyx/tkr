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

    function test_claim_usdc_partial_then_partial() public {
        bytes32 sid = bytes32(uint256(11));
        vm.prank(payer);
        escrow.open(sid, recipient, address(usdc), 100e6, uint64(block.timestamp + 1 days));

        // First receipt: 30 USDC
        vm.prank(recipient);
        escrow.claim(sid, 30e6, _signReceipt(sid, 30e6, payerKey));
        assertEq(usdc.balanceOf(recipient), 30e6);

        // Second receipt: cumulative 75 USDC (should pay out 45)
        vm.prank(recipient);
        escrow.claim(sid, 75e6, _signReceipt(sid, 75e6, payerKey));
        assertEq(usdc.balanceOf(recipient), 75e6);
    }

    function test_claim_revert_decreasing() public {
        bytes32 sid = bytes32(uint256(12));
        vm.prank(payer);
        escrow.open{value: 1 ether}(sid, recipient, address(0), 1 ether, uint64(block.timestamp + 1 days));

        vm.prank(recipient);
        escrow.claim(sid, 0.5 ether, _signReceipt(sid, 0.5 ether, payerKey));

        vm.prank(recipient);
        vm.expectRevert(MeshEscrow.AmountNotIncreasing.selector);
        escrow.claim(sid, 0.3 ether, _signReceipt(sid, 0.3 ether, payerKey));
    }

    function test_claim_revert_exceeds_deposit() public {
        bytes32 sid = bytes32(uint256(13));
        vm.prank(payer);
        escrow.open{value: 1 ether}(sid, recipient, address(0), 1 ether, uint64(block.timestamp + 1 days));

        vm.prank(recipient);
        vm.expectRevert(MeshEscrow.ExceedsDeposit.selector);
        escrow.claim(sid, 2 ether, _signReceipt(sid, 2 ether, payerKey));
    }

    function test_claim_revert_wrong_signer() public {
        bytes32 sid = bytes32(uint256(14));
        vm.prank(payer);
        escrow.open{value: 1 ether}(sid, recipient, address(0), 1 ether, uint64(block.timestamp + 1 days));

        // Recipient signs their own receipt (forgery attempt)
        vm.prank(recipient);
        vm.expectRevert(MeshEscrow.BadSignature.selector);
        escrow.claim(sid, 1 ether, _signReceipt(sid, 1 ether, recipientKey));
    }

    function test_claim_revert_missing_channel() public {
        bytes32 sid = bytes32(uint256(15));
        vm.prank(recipient);
        vm.expectRevert(MeshEscrow.ChannelMissing.selector);
        escrow.claim(sid, 1 ether, _signReceipt(sid, 1 ether, payerKey));
    }

    // ---------- Close ----------

    function test_close_refunds_unspent() public {
        bytes32 sid = bytes32(uint256(20));
        uint64 deadline = uint64(block.timestamp + 1 days);
        vm.prank(payer);
        escrow.open{value: 1 ether}(sid, recipient, address(0), 1 ether, deadline);

        // Recipient claims 0.4
        vm.prank(recipient);
        escrow.claim(sid, 0.4 ether, _signReceipt(sid, 0.4 ether, payerKey));

        // Warp past deadline, payer closes, gets remaining 0.6 back.
        vm.warp(deadline + 1);
        uint256 before = payer.balance;
        escrow.close(sid);
        assertEq(payer.balance - before, 0.6 ether);
        (address p,,,,, ) = escrow.getChannel(sid);
        assertEq(p, address(0)); // channel deleted
    }

    function test_close_revert_not_expired() public {
        bytes32 sid = bytes32(uint256(21));
        vm.prank(payer);
        escrow.open{value: 1 ether}(sid, recipient, address(0), 1 ether, uint64(block.timestamp + 1 days));
        vm.expectRevert(MeshEscrow.NotExpired.selector);
        escrow.close(sid);
    }

    function test_close_no_claim_full_refund() public {
        bytes32 sid = bytes32(uint256(22));
        uint64 deadline = uint64(block.timestamp + 1 days);
        vm.prank(payer);
        escrow.open{value: 1 ether}(sid, recipient, address(0), 1 ether, deadline);

        vm.warp(deadline + 1);
        uint256 before = payer.balance;
        escrow.close(sid);
        assertEq(payer.balance - before, 1 ether);
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
