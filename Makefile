.PHONY: clean default outdated print test

CARGOFLAGS =

default:
	cargo build -p function-renew-cert ${CARGOFLAGS}

clean:
	cargo clean

outdated:
	cargo-outdated

print:
	git status --porcelain

test:
	cargo test --workspace
	cargo clippy --workspace --tests --examples
