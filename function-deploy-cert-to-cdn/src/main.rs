#![deny(rust_2018_idioms, warnings)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(
	clippy::let_unit_value,
)]

use anyhow::Context;

function_worker::run! {
	"deploy-cert-to-cdn" => deploy_cert_to_cdn_main,
}

async fn deploy_cert_to_cdn_main(
	azure_subscription_id: &str,
	azure_auth: &azure::Auth,
	settings: &Settings<'_>,
	logger: &log2::Logger,
) -> anyhow::Result<()> {
	let user_agent: http::HeaderValue =
		concat!("github.com/Arnavion/acme-azure-function ", env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"))
		.parse().expect("hard-coded user agent is valid HeaderValue");

	let azure_key_vault_client = azure::key_vault::Client::new(
		&settings.azure_key_vault_name,
		&azure_auth,
		user_agent.clone(),
		logger,
	).context("could not initialize Azure KeyVault API client")?;

	let azure_management_client = azure::management::Client::new(
		&azure_subscription_id,
		&settings.azure_cdn_resource_group_name,
		&azure_auth,
		user_agent,
		logger,
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
				subscription_id: (&*azure_subscription_id).into(),
				resource_group: (&*settings.azure_key_vault_resource_group_name).into(),
				key_vault_name: (&*settings.azure_key_vault_name).into(),
				secret_name: (&*settings.azure_key_vault_certificate_name).into(),
				secret_version: expected_cdn_custom_domain_secret_version.into(),
			}, actual_cdn_custom_domain_secret)
		}
		else {
			logger.report_message("Nothing to do.");
			return Ok(());
		}
	};

	match actual_cdn_custom_domain_secret {
		Some(azure::management::cdn::CustomDomainSecret::KeyVault(actual_cdn_custom_domain_secret)) if expected_cdn_custom_domain_secret == actual_cdn_custom_domain_secret => {
			logger.report_message("CDN is up-to-date.");
			return Ok(());
		},

		Some(azure::management::cdn::CustomDomainSecret::Cdn) =>
			logger.report_message("CDN-managed cert will be replaced by user-managed cert."),

		_ => logger.report_message("User-managed cert will be deployed."),
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
struct Settings<'a> {
	/// The name of the Azure resource group that contains the Azure CDN
	#[serde(borrow)]
	azure_cdn_resource_group_name: std::borrow::Cow<'a, str>,

	/// The name of the Azure CDN profile
	#[serde(borrow)]
	azure_cdn_profile_name: std::borrow::Cow<'a, str>,

	/// The name of the Azure CDN endpoint in the Azure CDN profile
	#[serde(borrow)]
	azure_cdn_endpoint_name: std::borrow::Cow<'a, str>,

	/// The name of the custom domain resource in the Azure CDN endpoint
	#[serde(borrow)]
	azure_cdn_custom_domain_name: std::borrow::Cow<'a, str>,

	/// The name of the Azure resource group that contains the Azure KeyVault
	#[serde(borrow)]
	azure_key_vault_resource_group_name: std::borrow::Cow<'a, str>,

	/// The name of the Azure KeyVault
	#[serde(borrow)]
	azure_key_vault_name: std::borrow::Cow<'a, str>,

	/// The name of the certificate in the Azure KeyVault that contains the TLS certificate.
	///
	/// The new certificate will be uploaded here, and used for the custom domain.
	#[serde(borrow)]
	azure_key_vault_certificate_name: std::borrow::Cow<'a, str>,
}
