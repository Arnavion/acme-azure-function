#![deny(rust_2018_idioms, warnings)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(
	clippy::let_unit_value,
)]

use anyhow::Context;

function_worker::run! {
	"deploy-cert-to-cdn" => deploy_cert_to_cdn_main,
}

async fn deploy_cert_to_cdn_main(settings: std::sync::Arc<Settings>) -> anyhow::Result<()> {
	let azure_auth = azure::Auth::from_env(
		settings.azure_client_id.clone(),
		settings.azure_client_secret.clone(),
		settings.azure_tenant_id.clone(),
	)?;
	let azure_account = azure::Account::new(
		&settings.azure_subscription_id,
		&settings.azure_cdn_resource_group_name,
		&azure_auth,
		concat!("github.com/Arnavion/acme-azure-function ", env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION")),
	).context("could not initialize Azure API client")?;

	let (expected_cdn_custom_domain_secret_version, actual_cdn_custom_domain_secret) = {
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

	let expected_cdn_custom_domain_secret = azure::CdnCustomDomainKeyVaultSecret {
		subscription_id: (&*settings.azure_subscription_id).into(),
		resource_group: (&*settings.azure_key_vault_resource_group_name).into(),
		key_vault_name: (&*settings.azure_key_vault_name).into(),
		secret_name: (&*settings.azure_key_vault_certificate_name).into(),
		secret_version: expected_cdn_custom_domain_secret_version.into(),
	};

	match actual_cdn_custom_domain_secret {
		Some(azure::CdnCustomDomainSecret::KeyVault(actual_cdn_custom_domain_secret)) if expected_cdn_custom_domain_secret == actual_cdn_custom_domain_secret => {
			log2::report_message("CDN is up-to-date.");
			return Ok(());
		},

		Some(azure::CdnCustomDomainSecret::Cdn) =>
			log2::report_message("CDN-managed cert will be replaced by user-managed cert."),

		_ => log2::report_message("User-managed cert will be deployed."),
	}

	let () =
		azure_account.cdn_custom_domain_secret_set(
			&settings.azure_cdn_profile_name,
			&settings.azure_cdn_endpoint_name,
			&settings.azure_cdn_custom_domain_name,
			&expected_cdn_custom_domain_secret,
		).await?;

	Ok(())
}

#[derive(serde::Deserialize)]
struct Settings {
	/// The Azure subscription ID
	azure_subscription_id: String,

	/// The name of the Azure resource group that contains the Azure CDN
	azure_cdn_resource_group_name: String,

	/// The name of the Azure CDN profile
	azure_cdn_profile_name: String,

	/// The name of the Azure CDN endpoint in the Azure CDN profile
	azure_cdn_endpoint_name: String,

	/// The name of the custom domain resource in the Azure CDN endpoint
	azure_cdn_custom_domain_name: String,

	/// The name of the Azure resource group that contains the Azure KeyVault
	azure_key_vault_resource_group_name: String,

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
