#!/bin/sh
# Creates a symlink pointing outside the repository boundary.
# Called during fixture preparation to simulate an external symlink attack surface.
ln -sf /tmp/outside_target link_to_outside 2>/dev/null || true
