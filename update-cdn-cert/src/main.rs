#![deny(rust_2018_idioms, warnings)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(
	clippy::let_unit_value,
)]

use anyhow::Context;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
	function_host::run([
		("/update-cdn-cert", (|settings| Box::pin(run(settings)) as _) as fn(_) -> _),
	].iter().copied().collect()).await?;

	Ok(())
}

async fn run(settings: std::sync::Arc<Settings>) -> anyhow::Result<()> {
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

	let key_vault_certificate_version = {
		let certificate = azure_account.key_vault_certificate_get(&settings.azure_key_vault_name, &settings.azure_key_vault_certificate_name).await?;
		if let Some(certificate) = certificate {
			certificate.version
		}
		else {
			eprintln!("Nothing to do.");
			return Ok(());
		}
	};

	let cdn_custom_domain_secret =
		azure_account.cdn_custom_domain_secret_get(
			&settings.azure_cdn_profile_name,
			&settings.azure_cdn_endpoint_name,
			&settings.azure_cdn_custom_domain_name,
		).await?;

	if let Some(cdn_custom_domain_secret) = &cdn_custom_domain_secret {
		if cdn_custom_domain_secret.name == settings.azure_key_vault_certificate_name && cdn_custom_domain_secret.version == key_vault_certificate_version {
			eprintln!("Nothing to do.");
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
