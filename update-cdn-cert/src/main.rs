#![deny(rust_2018_idioms, warnings)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(
	clippy::let_unit_value,
)]

use anyhow::Context;

function_worker::run! {
	"update-cdn-cert" => update_cdn_cert_main,
}

async fn update_cdn_cert_main(settings: std::sync::Arc<Settings>) -> anyhow::Result<()> {
	let azure_auth = azure::Auth::from_env(
		settings.azure_client_id.clone(),
		settings.azure_client_secret.clone(),
		settings.azure_tenant_id.clone(),
	)?;
	let azure_account = azure::Account::new(
		&settings.azure_subscription_id,
		&settings.azure_resource_group_name,
		&azure_auth,
		concat!("github.com/Arnavion/acme-azure-function ", env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION")),
	).context("could not initialize Azure API client")?;

	let (key_vault_certificate_version, cdn_custom_domain_secret) = {
		let certificate_f = azure_account.key_vault_certificate_get(&settings.azure_key_vault_name, &settings.azure_key_vault_certificate_name);
		futures_util::pin_mut!(certificate_f);

		let secret_f = azure_account.cdn_custom_domain_secret_get(
			&settings.azure_cdn_profile_name,
			&settings.azure_cdn_endpoint_name,
			&settings.azure_cdn_custom_domain_name,
		);
		futures_util::pin_mut!(secret_f);

		let result =
			futures_util::future::try_select(certificate_f, secret_f).await
			.map_err(|err| err.factor_first().0)?;
		match result {
			futures_util::future::Either::Left((certificate, secret_f)) => {
				let certificate_version =
					if let Some(certificate) = certificate {
						certificate.version
					}
					else {
						log2::report_message("Nothing to do.");
						return Ok(());
					};

				(certificate_version, secret_f.await?)
			},

			futures_util::future::Either::Right((secret, certificate_f)) => {
				let certificate = certificate_f.await?;

				let certificate_version =
					if let Some(certificate) = certificate {
						certificate.version
					}
					else {
						log2::report_message("Nothing to do.");
						return Ok(());
					};

				(certificate_version, secret)
			},
		}
	};

	if let Some(cdn_custom_domain_secret) = cdn_custom_domain_secret {
		if cdn_custom_domain_secret.name == settings.azure_key_vault_certificate_name && cdn_custom_domain_secret.version == key_vault_certificate_version {
			log2::report_message("Nothing to do.");
			return Ok(());
		}
	}

	let () =
		azure_account.cdn_custom_domain_secret_set(
			&settings.azure_cdn_profile_name,
			&settings.azure_cdn_endpoint_name,
			&settings.azure_cdn_custom_domain_name,
			&settings.azure_key_vault_name,
			&settings.azure_key_vault_certificate_name,
			&key_vault_certificate_version,
		).await?;

	Ok(())
}

#[derive(serde::Deserialize)]
struct Settings {
	/// The Azure subscription ID
	azure_subscription_id: String,

	/// The name of the Azure resource group
	azure_resource_group_name: String,

	/// The name of the Azure CDN profile
	azure_cdn_profile_name: String,

	/// The name of the Azure CDN endpoint in the Azure CDN profile
	azure_cdn_endpoint_name: String,

	/// The name of the custom domain resource in the Azure CDN endpoint
	azure_cdn_custom_domain_name: String,

	/// The name of the Azure KeyVault
	azure_key_vault_name: String,

	/// The name of the certificate in the Azure KeyVault that contains the TLS certificate.
	///
	/// The new certificate will be uploaded here, and used for the custom domain.
	azure_key_vault_certificate_name: String,

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
