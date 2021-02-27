.PHONY: clean default

CARGOFLAGS =

default:
	cargo build -p acme -p update-cdn-cert ${CARGOFLAGS}

test:
	cargo clippy -p log2

	cargo clippy -p http-common

	set -euo pipefail; \
	for cdn in '' 'cdn,'; do \
		for dns in '' 'dns,'; do \
			for key_vault_cert in '' 'key_vault_cert,'; do \
				for key_vault_key in '' 'key_vault_key,'; do \
					for log_analytics in '' 'log_analytics,'; do \
						features="$$cdn$$dns$$key_vault_cert$$key_vault_key$$log_analytics"; \
						( \
							if ! cargo clippy --quiet --manifest-path ./azure/Cargo.toml --features "$$features"; then \
								>&2 echo "Failed to run clippy on azure with features '$$features'"; \
								exit 1; \
							fi; \
						) & :; \
					done; \
				done; \
			done; \
		done; \
	done; \
	wait $$(jobs -pr)

	cargo clippy -p function-worker

	cargo clippy -p acme

	cargo clippy -p update-cdn-cert

clean:
	cargo clean
