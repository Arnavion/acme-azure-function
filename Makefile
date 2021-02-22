.PHONY: clean default

CARGOFLAGS =

default:
	cargo build -p acme -p update-cdn-cert ${CARGOFLAGS}

test:
	set -euo pipefail; \
	for cdn in '' 'cdn,'; do \
		for dns in '' 'dns,'; do \
			for key_vault_cert in '' 'key_vault_cert,'; do \
				for key_vault_key in '' 'key_vault_key,'; do \
					features="$$cdn$$dns$$key_vault_cert$$key_vault_key"; \
					if ! cargo clippy --manifest-path ./azure/Cargo.toml --features "$$features"; then \
						>&2 echo "Failed to test azure with features '$$features'"; \
						exit 1; \
					fi \
				done; \
			done; \
		done; \
	done

	cargo clippy -p function-worker

	cargo clippy -p http-common

	cargo clippy -p acme

	cargo clippy -p update-cdn-cert

clean:
	cargo clean
