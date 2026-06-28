# cyberdeck-tui — developer shortcuts.
#
# The default inner loop is `cargo check` (see docs/CONTRIBUTING.md).
# `cargo test` for the WM crate spins up real PTYs, and on a busy dev
# box the spawned /bin/sh and /bin/cat children sometimes escape into
# the background. New test runs then deadlock in portable_pty waiting
# for a free PTY. The two targets below are the safe way to handle that.

.PHONY: clean-test-hang
clean-test-hang:
	# Kill any zombie cyberdeck_tui test binaries and their leaked
	# PTY children. Safe to run when nothing is hung — pkill exits 1
	# if no match, which we ignore so `make` keeps going.
	-pkill -9 -f 'target/debug/deps/cyberdeck_tui-'
	-pkill -9 -f 'cargo test'
	@echo "zombies reaped (if any were running)"
