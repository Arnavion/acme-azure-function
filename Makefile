.PHONY: clean default outdated print test

CARGOFLAGS =

default:
	cargo build -p acme-azure-function ${CARGOFLAGS}

clean:
	cargo clean

outdated:
	cargo-outdated

print:
	git status --porcelain

test:
	cargo test --workspace
	cargo clippy --workspace --tests --examples
