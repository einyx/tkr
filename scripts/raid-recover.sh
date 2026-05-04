#!/usr/bin/env bash
# scripts/raid-recover.sh — inspect + read-only recover the degraded `md0`
# RAID array on this host.
#
# Background: `sdb` carries the surviving `linux_raid_member` superblock
# (label "nas:0", UUID bb4d14c1-…). `sda` was repartitioned at some point,
# wiping its superblock — mdadm now sees only `sdb` and refuses to start
# the array. This script:
#
#   1. Reports the array's RAID level + device count (read-only).
#   2. Stops the kernel's inactive md0 so sdb is no longer "busy".
#   3. Force-assembles md0 in degraded mode from sdb alone.
#   4. Mounts the result read-only at /mnt/raid-recovery so we can see
#      what's on it without risk of writing.
#
# What this script does NOT do:
#   - Re-mirror sda into the array. That's destructive (overwrites sda2's
#     current contents) and only makes sense if RAID level is raid1.
#     The commented step at the bottom of this script does it once you've
#     decided. Read the examine output first.
#   - Reformat anything. Reclaiming the disks for plain ext4 is a separate
#     decision, not part of recovery.
#
# Usage:
#   sudo ./scripts/raid-recover.sh           # inspect + recover read-only
#   sudo ./scripts/raid-recover.sh --teardown  # umount + stop md0 (undo)

set -euo pipefail

if [ "${EUID:-$(id -u)}" -ne 0 ]; then
    echo "error: run with sudo (mdadm + mount need root)" >&2
    exit 1
fi

MD=/dev/md0
SURVIVING_MEMBER=/dev/sdb
MOUNTPOINT=/mnt/raid-recovery

cmd_teardown() {
    if mountpoint -q "$MOUNTPOINT"; then
        echo "umount $MOUNTPOINT"
        umount "$MOUNTPOINT"
    fi
    if [ -b "$MD" ]; then
        echo "mdadm --stop $MD"
        mdadm --stop "$MD" || true
    fi
    echo "done. sdb is released; nothing was written to disk."
}

if [ "${1:-}" = "--teardown" ]; then
    cmd_teardown
    exit 0
fi

echo "=== Step 1: examine $SURVIVING_MEMBER (read-only) ==="
mdadm --examine "$SURVIVING_MEMBER" \
    | grep -E 'Raid Level|Raid Devices|Active Devices|Working Devices|Update Time|Array UUID|Array State|Device Role' \
    || true
echo

# Pull the raid level out of the examine output so we can warn before
# attempting an assembly that physically can't recover (raid0).
LEVEL=$(mdadm --examine "$SURVIVING_MEMBER" 2>/dev/null \
    | awk -F: '/Raid Level/ {gsub(/ /,"",$2); print $2; exit}')
echo "→ detected RAID level: ${LEVEL:-unknown}"

case "$LEVEL" in
    raid1|raid5|raid6|raid10)
        echo "  → redundant level, degraded assembly may succeed."
        ;;
    raid0)
        echo "  → raid0 has no redundancy; sdb alone cannot reconstruct"
        echo "    the array. Recovery is impossible. Aborting."
        exit 2
        ;;
    "")
        echo "  → could not parse RAID level. Inspect output above and"
        echo "    decide manually. Aborting before any kernel changes."
        exit 2
        ;;
    *)
        echo "  → unfamiliar level '$LEVEL'. Aborting; investigate manually."
        exit 2
        ;;
esac
echo

echo "=== Step 2: release sdb from inactive md0 ==="
# /proc/mdstat shows md0 as inactive at boot because the kernel claimed
# sdb. `mdadm --stop` releases that association without touching the
# disk's superblock or data.
if grep -q '^md0' /proc/mdstat; then
    mdadm --stop "$MD"
    echo "stopped $MD"
else
    echo "$MD not present in /proc/mdstat — nothing to stop"
fi
echo

echo "=== Step 3: assemble degraded from $SURVIVING_MEMBER ==="
# --run = "start the array even though it's degraded". The kernel
# refuses by default to boot a one-leg mirror without explicit consent;
# --run is that consent.
mdadm --assemble --run "$MD" "$SURVIVING_MEMBER"
echo
cat /proc/mdstat
echo

echo "=== Step 4: read-only mount at $MOUNTPOINT ==="
mkdir -p "$MOUNTPOINT"
FSTYPE=$(blkid -o value -s TYPE "$MD" 2>/dev/null || echo unknown)
echo "filesystem on $MD: $FSTYPE"

# noload (ext4) / norecovery (xfs, btrfs) prevents the journal from being
# replayed on mount, which would write to disk even though we asked for
# read-only. Keep the recovery purely observational.
case "$FSTYPE" in
    ext2|ext3|ext4)
        mount -o ro,noload "$MD" "$MOUNTPOINT"
        ;;
    xfs|btrfs)
        mount -o ro,norecovery "$MD" "$MOUNTPOINT"
        ;;
    LVM2_member)
        echo "  → array contains LVM. Read-only mount needs lvm2 activation:"
        echo "    sudo vgchange -ay --activationmode partial"
        echo "    Then mount the LVs by name. Stopping here so you can decide."
        exit 0
        ;;
    *)
        echo "  → unknown FS type '$FSTYPE'. Mount manually and audit."
        exit 0
        ;;
esac

echo
echo "✓ mounted $MD on $MOUNTPOINT (read-only)"
echo "  contents:"
ls -lah "$MOUNTPOINT" | head -20
echo
df -h "$MOUNTPOINT"
echo
echo "Inspect at your leisure. When done, run:"
echo "  sudo $0 --teardown"
echo

# -----------------------------------------------------------------------
# DESTRUCTIVE — DO NOT UNCOMMENT WITHOUT READING THE EXAMINE OUTPUT FIRST
# -----------------------------------------------------------------------
# Only safe if (a) the RAID level above is raid1, AND (b) you've copied off
# anything you want from /mnt/raid-recovery, AND (c) you accept that the
# current contents of /dev/sda2 will be overwritten by the rebuild.
#
#     sudo umount /mnt/raid-recovery
#     sudo wipefs -a /dev/sda /dev/sda2          # clear the GPT we wrote
#     sudo mdadm --add /dev/md0 /dev/sda          # rebuild starts immediately
#     watch -n 5 cat /proc/mdstat                  # ETA + sync %
#
# Sync time on a 1.8 TB raid1 over SATA is roughly 6-10 hours.
# -----------------------------------------------------------------------
