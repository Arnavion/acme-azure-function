.PHONY: clean default

CARGOFLAGS =

default:
	cargo build -p function-renew-cert ${CARGOFLAGS}

test:
	cargo clippy --all

clean:
	cargo clean
