# cyberdeck-tui — developer shortcuts.
#
# The default inner loop is `cargo check` (see docs/CONTRIBUTING.md).
# `cargo test` for the WM crate spins up real PTYs, and on a busy dev
# box the spawned /bin/sh and /bin/cat children sometimes escape into
# the background. New test runs then deadlock in portable_pty waiting
# for a free PTY.
#
# Two layers of defence:
#
#   1. `make test` is the canonical way to run `cargo test` on this
#      repo. It delegates to `scripts/safe-test`, which mechanically
#      refuses blanket form, auto-injects `--test-threads=1` for
#      `cyberdeck-tui` runs, and caps the wall clock at 600 s so a
#      misbehaving test can never deadlock the developer. Anything in
#      `docs/CONTRIBUTING.md` or the plans that says "run
#      `cargo test …`" really means `make test …`.
#
#   2. `make clean-test-hang` is the kill-switch when something has
#      already wedged the PTY pool (backgrounded /bin/cat from a
#      previous test run, etc.).

.PHONY: test
test:
	@scripts/safe-test $(ARGS)

# Backwards-compat: `make test-ci` runs the workspace-wide CI-parity
# suite (the only sanctioned blanket form). Pass through `--ci` and
# `--test-threads=1` so the wrapper accepts it.
.PHONY: test-ci
test-ci:
	@scripts/safe-test --ci --workspace -- --test-threads=1

.PHONY: clean-test-hang
clean-test-hang:
	# Kill any zombie cyberdeck_tui test binaries and their leaked
	# PTY children. Safe to run when nothing is hung — pkill exits 1
	# if no match, which we ignore so `make` keeps going.
	-pkill -9 -f 'target/debug/deps/cyberdeck_tui-'
	-pkill -9 -f 'cargo test'
	@echo "zombies reaped (if any were running)"
