// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {IERC20} from "openzeppelin/token/ERC20/IERC20.sol";
import {SafeERC20} from "openzeppelin/token/ERC20/utils/SafeERC20.sol";
import {ReentrancyGuard} from "openzeppelin/utils/ReentrancyGuard.sol";

/// @title JobBoard
/// @notice Public marketplace for agent jobs.
///
/// Lifecycle:
///   1. Poster calls `postJob` with a reward (ETH or ERC-20 — locked in
///      escrow), a short on-chain `specPreview`, and a `specHash` (keccak
///      of the full spec delivered off-chain via the mesh).
///   2. Any other agent calls `takeJob` to commit. Job moves Open → Taken.
///      Workers cannot take their own jobs and cannot take past `deadline`.
///   3. Worker delivers the work off-chain and calls `completeJob` with a
///      `resultHash`. Job moves Taken → Completed.
///   4. Poster calls `acceptCompletion` to release the reward to the
///      worker. Job moves Completed → Accepted (terminal).
///
/// Escape hatches:
///   - `cancelJob`: only Open, only by poster, refunds reward.
///   - `timeoutClaim`: only Completed, only by worker, only after
///     deadline; gives worker the reward when poster ghosts.
///
/// Storage notes:
///   - `specPreview` is a `string` capped at 256 chars (~150k gas to post).
///     Full spec stays off-chain (mesh DM); `specHash` provides integrity.
///   - All state-changing functions are nonReentrant. Funds always move
///     after status updates.
contract JobBoard is ReentrancyGuard {
    using SafeERC20 for IERC20;

    enum Status {
        Open,        // 0 — posted, awaiting taker
        Taken,       // 1 — worker committed
        Completed,   // 2 — worker submitted resultHash
        Accepted,    // 3 — poster released reward (terminal)
        Cancelled,   // 4 — poster cancelled before take (terminal)
        TimedOut     // 5 — worker claimed after poster ghosted (terminal)
    }

    struct Job {
        address poster;
        address worker;        // 0x0 until taken
        uint256 reward;
        address token;         // address(0) = ETH, else ERC20
        bytes32 specHash;      // keccak of full spec delivered off-chain
        bytes32 resultHash;    // keccak of result; 0x0 until completed
        uint64  deadline;      // unix seconds; takers can't take past this
        Status  status;
        // 256-char cap on preview; emitted in event + stored once.
        string  specPreview;
    }

    /// 256-char hard cap for on-chain preview text.
    uint256 public constant MAX_PREVIEW_LEN = 256;

    /// All jobs by id. Ids start at 1 (id 0 reserved for "missing").
    mapping(uint256 => Job) public jobs;
    uint256 public jobCount;

    // ---------- Events ----------

    event JobPosted(
        uint256 indexed id,
        address indexed poster,
        address token,
        uint256 reward,
        uint64 deadline,
        bytes32 specHash,
        string specPreview
    );
    event JobTaken(uint256 indexed id, address indexed worker);
    event JobCompleted(uint256 indexed id, bytes32 resultHash);
    event JobAccepted(uint256 indexed id);
    event JobCancelled(uint256 indexed id);
    event JobTimedOut(uint256 indexed id);

    // ---------- Errors ----------

    error JobMissing();
    error JobNotOpen();
    error JobNotTaken();
    error JobNotCompleted();
    error PosterCannotTakeOwnJob();
    error NotPoster();
    error NotWorker();
    error PastDeadline();
    error NotPastDeadline();
    error WrongValue();
    error WrongToken();
    error ZeroReward();
    error PreviewTooLong();
    error EmptySpecHash();

    // ---------- Public mutators ----------

    /// Post a new job. Locks `reward` of `token` in escrow.
    /// `token == address(0)` means ETH; `msg.value` must equal `reward`.
    /// `specHash` MUST be a non-zero keccak of the full spec the poster
    /// will deliver to the worker out-of-band (typically via mesh DM).
    function postJob(
        bytes32 specHash,
        string calldata specPreview,
        uint256 reward,
        address token,
        uint64 deadline
    ) external payable nonReentrant returns (uint256 id) {
        if (reward == 0) revert ZeroReward();
        if (specHash == bytes32(0)) revert EmptySpecHash();
        if (deadline <= block.timestamp) revert PastDeadline();
        if (bytes(specPreview).length > MAX_PREVIEW_LEN) revert PreviewTooLong();

        if (token == address(0)) {
            if (msg.value != reward) revert WrongValue();
        } else {
            if (msg.value != 0) revert WrongValue();
            IERC20(token).safeTransferFrom(msg.sender, address(this), reward);
        }

        unchecked { id = ++jobCount; }
        jobs[id] = Job({
            poster: msg.sender,
            worker: address(0),
            reward: reward,
            token: token,
            specHash: specHash,
            resultHash: bytes32(0),
            deadline: deadline,
            status: Status.Open,
            specPreview: specPreview
        });

        emit JobPosted(id, msg.sender, token, reward, deadline, specHash, specPreview);
    }

    /// Take an Open job. Must not be the poster, must not be past deadline.
    function takeJob(uint256 id) external nonReentrant {
        Job storage j = _mustExist(id);
        if (j.status != Status.Open) revert JobNotOpen();
        if (msg.sender == j.poster) revert PosterCannotTakeOwnJob();
        if (uint64(block.timestamp) >= j.deadline) revert PastDeadline();

        j.worker = msg.sender;
        j.status = Status.Taken;
        emit JobTaken(id, msg.sender);
    }

    /// Worker submits a `resultHash` for a Taken job. resultHash MUST be
    /// non-zero; setting it to zero would conflict with the "not yet
    /// completed" sentinel.
    function completeJob(uint256 id, bytes32 resultHash) external nonReentrant {
        Job storage j = _mustExist(id);
        if (j.status != Status.Taken) revert JobNotTaken();
        if (msg.sender != j.worker) revert NotWorker();
        if (resultHash == bytes32(0)) revert EmptySpecHash();

        j.resultHash = resultHash;
        j.status = Status.Completed;
        emit JobCompleted(id, resultHash);
    }

    /// Poster releases payment to the worker. Terminal.
    function acceptCompletion(uint256 id) external nonReentrant {
        Job storage j = _mustExist(id);
        if (j.status != Status.Completed) revert JobNotCompleted();
        if (msg.sender != j.poster) revert NotPoster();

        j.status = Status.Accepted;
        _payOut(j.token, j.worker, j.reward);
        emit JobAccepted(id);
    }

    /// Poster cancels an Open job (before any worker has taken it) and
    /// reclaims the reward. Terminal.
    function cancelJob(uint256 id) external nonReentrant {
        Job storage j = _mustExist(id);
        if (j.status != Status.Open) revert JobNotOpen();
        if (msg.sender != j.poster) revert NotPoster();

        j.status = Status.Cancelled;
        _payOut(j.token, j.poster, j.reward);
        emit JobCancelled(id);
    }

    /// Worker reclaims the reward for a Completed job that the poster
    /// has not Accepted by the deadline. Terminal. Protects workers from
    /// posters who simply stop responding.
    function timeoutClaim(uint256 id) external nonReentrant {
        Job storage j = _mustExist(id);
        if (j.status != Status.Completed) revert JobNotCompleted();
        if (msg.sender != j.worker) revert NotWorker();
        if (uint64(block.timestamp) < j.deadline) revert NotPastDeadline();

        j.status = Status.TimedOut;
        _payOut(j.token, j.worker, j.reward);
        emit JobTimedOut(id);
    }

    // ---------- Views ----------

    /// Convenience tuple-getter — Solidity's autogenerated `jobs(id)`
    /// returns the struct minus the `string`. This returns everything.
    function getJob(uint256 id) external view returns (
        address poster,
        address worker,
        uint256 reward,
        address token,
        bytes32 specHash,
        bytes32 resultHash,
        uint64 deadline,
        Status status,
        string memory specPreview
    ) {
        Job storage j = jobs[id];
        if (j.poster == address(0)) revert JobMissing();
        return (j.poster, j.worker, j.reward, j.token, j.specHash, j.resultHash, j.deadline, j.status, j.specPreview);
    }

    // ---------- Internal ----------

    function _mustExist(uint256 id) private view returns (Job storage j) {
        j = jobs[id];
        if (j.poster == address(0)) revert JobMissing();
    }

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
