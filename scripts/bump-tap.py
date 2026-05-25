#!/usr/bin/env python3
"""Update einyx/homebrew-tap Formula/jkr.rb with a new version + SHAs."""
import re, sys

path, ver, sha_arm_mac, sha_x86_mac, sha_arm_lnx, sha_x86_lnx = sys.argv[1:]
src = open(path).read()
src = re.sub(r'version "[^"]+"', f'version "{ver}"', src, count=1)
shas = iter([sha_arm_mac, sha_x86_mac, sha_arm_lnx, sha_x86_lnx])
src = re.sub(r'sha256 "[a-f0-9]+"', lambda m: f'sha256 "{next(shas)}"', src)
open(path, 'w').write(src)
print(f"Bumped {path} to v{ver}")
