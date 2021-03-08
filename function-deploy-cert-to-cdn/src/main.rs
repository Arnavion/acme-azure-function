#![deny(rust_2018_idioms, warnings)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(
	clippy::let_unit_value,
)]

use anyhow::Context;

function_worker::run! {
	"deploy-cert-to-cdn" => deploy_cert_to_cdn_main,
}

async fn deploy_cert_to_cdn_main(settings: std::rc::Rc<Settings>) -> anyhow::Result<()> {
	let azure_auth = azure::Auth::from_env(
		settings.azure_client_id.clone(),
		settings.azure_client_secret.clone(),
		settings.azure_tenant_id.clone(),
	)?;

	let azure_key_vault_client = azure::key_vault::Client::new(
		&settings.azure_key_vault_name,
		&azure_auth,
		concat!("github.com/Arnavion/acme-azure-function ", env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION")),
	).context("could not initialize Azure KeyVault API client")?;

	let azure_management_client = azure::management::Client::new(
		&settings.azure_subscription_id,
		&settings.azure_cdn_resource_group_name,
		&azure_auth,
		concat!("github.com/Arnavion/acme-azure-function ", env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION")),
	).context("could not initialize Azure Management API client")?;

	let (expected_cdn_custom_domain_secret, actual_cdn_custom_domain_secret) = {
		let certificate_f = azure_key_vault_client.certificate_get(&settings.azure_key_vault_certificate_name);
		futures_util::pin_mut!(certificate_f);

		let secret_f = azure_management_client.cdn_custom_domain_secret_get(
			&settings.azure_cdn_profile_name,
			&settings.azure_cdn_endpoint_name,
			&settings.azure_cdn_custom_domain_name,
		);
		futures_util::pin_mut!(secret_f);

		let result = futures_util::future::select(certificate_f, secret_f).await;
		let result = match result {
			futures_util::future::Either::Left((certificate, secret_f)) => {
				let certificate = certificate?;
				futures_util::future::OptionFuture::from(certificate.map(|certificate| async {
					let secret = secret_f.await?;
					Ok::<_, anyhow::Error>((certificate.version, secret))
				})).await.transpose()?
			},
			futures_util::future::Either::Right((secret, certificate_f)) => {
				let secret = secret?;
				let certificate = certificate_f.await?;
				certificate.map(|certificate| (certificate.version, secret))
			},
		};
		if let Some((expected_cdn_custom_domain_secret_version, actual_cdn_custom_domain_secret)) = result {
			(azure::management::cdn::CustomDomainKeyVaultSecret {
				subscription_id: (&*settings.azure_subscription_id).into(),
				resource_group: (&*settings.azure_key_vault_resource_group_name).into(),
				key_vault_name: (&*settings.azure_key_vault_name).into(),
				secret_name: (&*settings.azure_key_vault_certificate_name).into(),
				secret_version: expected_cdn_custom_domain_secret_version.into(),
			}, actual_cdn_custom_domain_secret)
		}
		else {
			log2::report_message("Nothing to do.");
			return Ok(());
		}
	};

	match actual_cdn_custom_domain_secret {
		Some(azure::management::cdn::CustomDomainSecret::KeyVault(actual_cdn_custom_domain_secret)) if expected_cdn_custom_domain_secret == actual_cdn_custom_domain_secret => {
			log2::report_message("CDN is up-to-date.");
			return Ok(());
		},

		Some(azure::management::cdn::CustomDomainSecret::Cdn) =>
			log2::report_message("CDN-managed cert will be replaced by user-managed cert."),

		_ => log2::report_message("User-managed cert will be deployed."),
	}

	let () =
		azure_management_client.cdn_custom_domain_secret_set(
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