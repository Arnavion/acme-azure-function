.PHONY: clean default

CARGOFLAGS =

default:
	cargo build --release -p acme -p update-cdn-cert ${CARGOFLAGS}

debug:
	cargo build -p acme -p update-cdn-cert ${CARGOFLAGS}

test:
	cargo test --all
	cargo clippy --all

clean:
	cargo clean
