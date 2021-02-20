#![deny(rust_2018_idioms, warnings)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(
	clippy::default_trait_access,
	clippy::let_unit_value,
	clippy::let_and_return,
	clippy::too_many_arguments,
	clippy::too_many_lines,
)]

mod proto;

use anyhow::Context;

function_worker::run! {
	"acme" => acme_main,
}

async fn acme_main(settings: std::sync::Arc<Settings>) -> anyhow::Result<()> {
	let azure_auth = azure::Auth::from_env(
		settings.azure_client_id.as_deref(),
		settings.azure_client_secret.as_deref(),
		settings.azure_tenant_id.as_deref(),
	)?;
	let mut azure_account = azure::Account::new(
		&settings.azure_subscription_id,
		&settings.azure_resource_group_name,
		&azure_auth,
		concat!("github.com/Arnavion/acme-azure-function ", env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION")),
	).context("could not initialize Azure API client")?;

	let need_new_certificate = {
		let certificate = azure_account.key_vault_certificate_get(&settings.azure_key_vault_name, &settings.azure_key_vault_certificate_name).await?;
		let need_new_certificate =
			certificate.map_or(true, |certificate| certificate.not_after < chrono::Utc::now() + chrono::Duration::days(30));
		need_new_certificate
	};
	if !need_new_certificate {
		eprintln!("Certificate does not need to be renewed");
		return Ok(());
	}

	let account_key = {
		let account_key = azure_account.key_vault_key_get(&settings.azure_key_vault_name, &settings.azure_key_vault_acme_account_key_name).await?;
		if let Some(account_key) = account_key {
			account_key
		}
		else {
			let account_key = azure_account.key_vault_key_create(&settings.azure_key_vault_name, &settings.azure_key_vault_acme_account_key_name).await?;
			account_key
		}
	};

	let mut acme_account = proto::Account::new(
		&settings.acme_directory_url,
		&settings.acme_contact_url,
		&mut azure_account,
		&account_key,
		concat!("github.com/Arnavion/acme-azure-function ", env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION")),
	).await.context("could not initialize ACME API client")?;

	let domain_name = format!("*.{}", settings.top_level_domain_name);

	let mut acme_order = acme_account.place_order(&domain_name).await?;

	let certificates = {
		let certificate = loop {
			match acme_order {
				proto::Order::Pending(pending) => {
					let () =
						acme_account.azure_account.dns_txt_record_create(
							&settings.top_level_domain_name,
							"_acme-challenge",
							&pending.dns_txt_record_content,
						).await?;

					// Don't use `?` to fail immediately. Delete the TXT record first.
					let new_acme_order = acme_account.complete_authorization(pending).await;

					let () =
						acme_account.azure_account.dns_txt_record_delete(
							&settings.top_level_domain_name,
							"_acme-challenge",
						).await?;

					acme_order = proto::Order::Ready(new_acme_order?);
				},

				proto::Order::Ready(ready) => {
					let csr =
						acme_account.azure_account.key_vault_csr_create(
							&settings.azure_key_vault_name,
							&settings.azure_key_vault_certificate_name,
							&domain_name,
						).await?;

					acme_order = proto::Order::Valid(acme_account.finalize_order(ready, &csr).await?);
				},

				proto::Order::Valid(valid) =>
					break acme_account.download_certificate(valid).await?,
			}
		};

		let mut certificates = vec![];
		let mut current_cert = String::new();
		let mut lines = certificate.lines();

		if lines.next() != Some("-----BEGIN CERTIFICATE-----") {
			return Err(anyhow::anyhow!("malformed PEM: does not start with BEGIN CERTIFICATE"));
		}

		for line in lines {
			if line == "-----END CERTIFICATE-----" {
				certificates.push(std::mem::take(&mut current_cert));
			}
			else if line == "-----BEGIN CERTIFICATE-----" {
				if !current_cert.is_empty() {
					return Err(anyhow::anyhow!("malformed PEM: BEGIN CERTIFICATE without prior END CERTIFICATE"));
				}
			}
			else {
				current_cert.push_str(line);
			}
		}
		if !current_cert.is_empty() {
			return Err(anyhow::anyhow!("malformed PEM: BEGIN CERTIFICATE without corresponding END CERTIFICATE"));
		}

		certificates
	};

	let () =
		acme_account.azure_account.key_vault_certificate_merge(
			&settings.azure_key_vault_name,
			&settings.azure_key_vault_certificate_name,
			&certificates,
		).await?;

	eprintln!("Certificate has been renewed");

	Ok(())
}

#[derive(serde::Deserialize)]
struct Settings {
	/// The directory URL of the ACME server
	acme_directory_url: String,

	/// The contact URL of the ACME account
	acme_contact_url: String,

	/// The Azure subscription ID
	azure_subscription_id: String,

	/// The name of the Azure resource group
	azure_resource_group_name: String,

	/// The name of the Azure KeyVault
	azure_key_vault_name: String,

	/// The name of the KeyVault secret that contains the ACME account key.
	///
	/// A new key will be generated and uploaded if this secret does not already exist.
	azure_key_vault_acme_account_key_name: String,

	/// The name of the certificate in the Azure KeyVault that contains the TLS certificate.
	///
	/// The new certificate will be uploaded here, and used for the custom domain.
	azure_key_vault_certificate_name: String,

	/// The domain name to request the TLS certificate for
	top_level_domain_name: String,

	/// The application ID of the service principal that this Function should use to access Azure resources.
	///
	/// Only needed for local testing; the final released Function should be set to use the Function app MSI.
	azure_client_id: Option<String>,

	/// The password of the service principal that this Function should use to access Azure resources.
	///
	/// Only needed for local testing; the final released Function should be set to use the Function app MSI.
	azure_client_secret: Option<String>,

	/// The tenant ID of the service principal that this Function should use to access Azure resources.
	///
	/// Only needed for local testing; the final released Function should be set to use the Function app MSI.
	azure_tenant_id: Option<String>,
}
