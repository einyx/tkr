// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {JobBoard} from "../src/JobBoard.sol";
import {ERC20} from "openzeppelin/token/ERC20/ERC20.sol";

contract MockUSDC is ERC20 {
    constructor() ERC20("Mock USDC", "mUSDC") {}
    function mint(address to, uint256 amount) external { _mint(to, amount); }
    function decimals() public pure override returns (uint8) { return 6; }
}

contract JobBoardTest is Test {
    JobBoard internal board;
    MockUSDC internal usdc;

    address internal poster = address(0xA11CE);
    address internal worker = address(0xB0B);
    address internal stranger = address(0xCAFE);

    bytes32 constant SPEC_HASH = keccak256("summarize this URL ...");
    bytes32 constant RESULT_HASH = keccak256("here is the summary");
    string constant PREVIEW = "summarize URL X in 200 words";

    function setUp() public {
        board = new JobBoard();
        usdc = new MockUSDC();
        vm.deal(poster, 100 ether);
        vm.deal(worker, 1 ether);
        vm.deal(stranger, 1 ether);
        usdc.mint(poster, 1_000_000e6);
        vm.prank(poster);
        usdc.approve(address(board), type(uint256).max);
    }

    function _post_eth(uint256 reward, uint64 deadline) internal returns (uint256 id) {
        vm.prank(poster);
        id = board.postJob{value: reward}(SPEC_HASH, PREVIEW, reward, address(0), deadline);
    }

    // ---------- Post ----------

    function test_post_eth() public {
        uint256 id = _post_eth(1 ether, uint64(block.timestamp + 1 days));
        assertEq(id, 1);
        (address p,,uint256 r,,,,,,) = board.getJob(1);
        assertEq(p, poster);
        assertEq(r, 1 ether);
        assertEq(address(board).balance, 1 ether);
    }

    function test_post_usdc() public {
        vm.prank(poster);
        uint256 id = board.postJob(SPEC_HASH, PREVIEW, 50e6, address(usdc), uint64(block.timestamp + 1 days));
        assertEq(id, 1);
        assertEq(usdc.balanceOf(address(board)), 50e6);
    }

    function test_post_revert_zero_reward() public {
        vm.prank(poster);
        vm.expectRevert(JobBoard.ZeroReward.selector);
        board.postJob{value: 0}(SPEC_HASH, PREVIEW, 0, address(0), uint64(block.timestamp + 1 days));
    }

    function test_post_revert_empty_spec_hash() public {
        vm.prank(poster);
        vm.expectRevert(JobBoard.EmptySpecHash.selector);
        board.postJob{value: 1 ether}(bytes32(0), PREVIEW, 1 ether, address(0), uint64(block.timestamp + 1 days));
    }

    function test_post_revert_past_deadline() public {
        vm.prank(poster);
        vm.expectRevert(JobBoard.PastDeadline.selector);
        board.postJob{value: 1 ether}(SPEC_HASH, PREVIEW, 1 ether, address(0), uint64(block.timestamp));
    }

    function test_post_revert_value_mismatch_eth() public {
        vm.prank(poster);
        vm.expectRevert(JobBoard.WrongValue.selector);
        board.postJob{value: 0.5 ether}(SPEC_HASH, PREVIEW, 1 ether, address(0), uint64(block.timestamp + 1 days));
    }

    function test_post_revert_value_with_token() public {
        vm.prank(poster);
        vm.expectRevert(JobBoard.WrongValue.selector);
        board.postJob{value: 0.1 ether}(SPEC_HASH, PREVIEW, 50e6, address(usdc), uint64(block.timestamp + 1 days));
    }

    function test_post_revert_preview_too_long() public {
        bytes memory big = new bytes(257);
        for (uint256 i = 0; i < big.length; i++) big[i] = bytes1("x");
        vm.prank(poster);
        vm.expectRevert(JobBoard.PreviewTooLong.selector);
        board.postJob{value: 1 ether}(SPEC_HASH, string(big), 1 ether, address(0), uint64(block.timestamp + 1 days));
    }

    // ---------- Take ----------

    function test_take_happy_path() public {
        uint256 id = _post_eth(1 ether, uint64(block.timestamp + 1 days));
        vm.prank(worker);
        board.takeJob(id);
        (,address w,,,,,,JobBoard.Status s,) = board.getJob(id);
        assertEq(w, worker);
        assertEq(uint8(s), uint8(JobBoard.Status.Taken));
    }

    function test_take_revert_poster_cannot_take_own() public {
        uint256 id = _post_eth(1 ether, uint64(block.timestamp + 1 days));
        vm.prank(poster);
        vm.expectRevert(JobBoard.PosterCannotTakeOwnJob.selector);
        board.takeJob(id);
    }

    function test_take_revert_past_deadline() public {
        uint256 id = _post_eth(1 ether, uint64(block.timestamp + 1 days));
        vm.warp(block.timestamp + 2 days);
        vm.prank(worker);
        vm.expectRevert(JobBoard.PastDeadline.selector);
        board.takeJob(id);
    }

    function test_take_revert_already_taken() public {
        uint256 id = _post_eth(1 ether, uint64(block.timestamp + 1 days));
        vm.prank(worker);
        board.takeJob(id);
        vm.prank(stranger);
        vm.expectRevert(JobBoard.JobNotOpen.selector);
        board.takeJob(id);
    }

    function test_take_revert_missing() public {
        vm.prank(worker);
        vm.expectRevert(JobBoard.JobMissing.selector);
        board.takeJob(99);
    }

    // ---------- Complete + Accept ----------

    function test_complete_then_accept_pays_worker() public {
        uint256 id = _post_eth(1 ether, uint64(block.timestamp + 1 days));
        vm.prank(worker);
        board.takeJob(id);
        vm.prank(worker);
        board.completeJob(id, RESULT_HASH);
        uint256 before = worker.balance;
        vm.prank(poster);
        board.acceptCompletion(id);
        assertEq(worker.balance - before, 1 ether);
        (,,,,,bytes32 rh,, JobBoard.Status s,) = board.getJob(id);
        assertEq(rh, RESULT_HASH);
        assertEq(uint8(s), uint8(JobBoard.Status.Accepted));
    }

    function test_complete_revert_not_worker() public {
        uint256 id = _post_eth(1 ether, uint64(block.timestamp + 1 days));
        vm.prank(worker);
        board.takeJob(id);
        vm.prank(stranger);
        vm.expectRevert(JobBoard.NotWorker.selector);
        board.completeJob(id, RESULT_HASH);
    }

    function test_complete_revert_not_taken() public {
        uint256 id = _post_eth(1 ether, uint64(block.timestamp + 1 days));
        vm.prank(worker);
        vm.expectRevert(JobBoard.JobNotTaken.selector);
        board.completeJob(id, RESULT_HASH);
    }

    function test_complete_revert_empty_result() public {
        uint256 id = _post_eth(1 ether, uint64(block.timestamp + 1 days));
        vm.prank(worker);
        board.takeJob(id);
        vm.prank(worker);
        vm.expectRevert(JobBoard.EmptySpecHash.selector); // reused error: empty hash
        board.completeJob(id, bytes32(0));
    }

    function test_accept_revert_not_poster() public {
        uint256 id = _post_eth(1 ether, uint64(block.timestamp + 1 days));
        vm.prank(worker);
        board.takeJob(id);
        vm.prank(worker);
        board.completeJob(id, RESULT_HASH);
        vm.prank(stranger);
        vm.expectRevert(JobBoard.NotPoster.selector);
        board.acceptCompletion(id);
    }

    function test_accept_revert_not_completed() public {
        uint256 id = _post_eth(1 ether, uint64(block.timestamp + 1 days));
        vm.prank(worker);
        board.takeJob(id);
        // Skip complete; try to accept directly.
        vm.prank(poster);
        vm.expectRevert(JobBoard.JobNotCompleted.selector);
        board.acceptCompletion(id);
    }

    // ---------- Cancel ----------

    function test_cancel_refunds_poster() public {
        uint256 id = _post_eth(1 ether, uint64(block.timestamp + 1 days));
        uint256 before = poster.balance;
        vm.prank(poster);
        board.cancelJob(id);
        assertEq(poster.balance - before, 1 ether);
        (,,,,,,, JobBoard.Status s,) = board.getJob(id);
        assertEq(uint8(s), uint8(JobBoard.Status.Cancelled));
    }

    function test_cancel_revert_not_poster() public {
        uint256 id = _post_eth(1 ether, uint64(block.timestamp + 1 days));
        vm.prank(stranger);
        vm.expectRevert(JobBoard.NotPoster.selector);
        board.cancelJob(id);
    }

    function test_cancel_revert_after_take() public {
        uint256 id = _post_eth(1 ether, uint64(block.timestamp + 1 days));
        vm.prank(worker);
        board.takeJob(id);
        vm.prank(poster);
        vm.expectRevert(JobBoard.JobNotOpen.selector);
        board.cancelJob(id);
    }

    // ---------- Timeout claim ----------

    function test_timeout_claim_pays_worker_after_deadline() public {
        uint64 deadline = uint64(block.timestamp + 1 days);
        uint256 id = _post_eth(1 ether, deadline);
        vm.prank(worker);
        board.takeJob(id);
        vm.prank(worker);
        board.completeJob(id, RESULT_HASH);

        // Poster ghosts. After deadline, worker can claim.
        vm.warp(deadline + 1);
        uint256 before = worker.balance;
        vm.prank(worker);
        board.timeoutClaim(id);
        assertEq(worker.balance - before, 1 ether);
        (,,,,,,, JobBoard.Status s,) = board.getJob(id);
        assertEq(uint8(s), uint8(JobBoard.Status.TimedOut));
    }

    function test_timeout_claim_revert_before_deadline() public {
        uint256 id = _post_eth(1 ether, uint64(block.timestamp + 1 days));
        vm.prank(worker);
        board.takeJob(id);
        vm.prank(worker);
        board.completeJob(id, RESULT_HASH);
        vm.prank(worker);
        vm.expectRevert(JobBoard.NotPastDeadline.selector);
        board.timeoutClaim(id);
    }

    function test_timeout_claim_revert_not_worker() public {
        uint64 deadline = uint64(block.timestamp + 1 days);
        uint256 id = _post_eth(1 ether, deadline);
        vm.prank(worker);
        board.takeJob(id);
        vm.prank(worker);
        board.completeJob(id, RESULT_HASH);
        vm.warp(deadline + 1);
        vm.prank(stranger);
        vm.expectRevert(JobBoard.NotWorker.selector);
        board.timeoutClaim(id);
    }

    // ---------- USDC end-to-end ----------

    function test_usdc_full_lifecycle() public {
        vm.prank(poster);
        uint256 id = board.postJob(SPEC_HASH, PREVIEW, 100e6, address(usdc), uint64(block.timestamp + 1 days));
        vm.prank(worker);
        board.takeJob(id);
        vm.prank(worker);
        board.completeJob(id, RESULT_HASH);
        uint256 before = usdc.balanceOf(worker);
        vm.prank(poster);
        board.acceptCompletion(id);
        assertEq(usdc.balanceOf(worker) - before, 100e6);
    }

    // ---------- jobCount / id increment ----------

    function test_ids_start_at_one_and_increment() public {
        assertEq(board.jobCount(), 0);
        uint256 a = _post_eth(0.1 ether, uint64(block.timestamp + 1 days));
        uint256 b = _post_eth(0.2 ether, uint64(block.timestamp + 1 days));
        uint256 c = _post_eth(0.3 ether, uint64(block.timestamp + 1 days));
        assertEq(a, 1);
        assertEq(b, 2);
        assertEq(c, 3);
        assertEq(board.jobCount(), 3);
    }
}
