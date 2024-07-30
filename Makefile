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
	# Ref: https://github.com/rust-lang/rust-clippy/issues/12270
	cargo clippy --workspace --tests --examples -- -A 'clippy::lint_groups_priority'
	cargo machete
