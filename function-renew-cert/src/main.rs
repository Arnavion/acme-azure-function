#![deny(rust_2018_idioms, warnings)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(
	clippy::default_trait_access,
	clippy::let_unit_value,
	clippy::let_and_return,
	clippy::too_many_arguments,
	clippy::too_many_lines,
)]

use anyhow::Context;

function_worker::run! {
	"renew-cert" => renew_cert_main,
}

async fn renew_cert_main(
	azure_subscription_id: &str,
	azure_auth: &azure::Auth,
	settings: &Settings<'_>,
	logger: &log2::Logger,
) -> anyhow::Result<&'static str> {
	let user_agent: http::HeaderValue =
		concat!("github.com/Arnavion/acme-azure-function ", env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"))
		.parse().expect("hard-coded user agent is valid HeaderValue");

	let azure_key_vault_client = azure::key_vault::Client::new(
		&settings.azure_key_vault_name,
		&azure_auth,
		user_agent.clone(),
		logger,
	).context("could not initialize Azure KeyVault API client")?;

	let need_new_certificate = {
		let certificate = azure_key_vault_client.certificate_get(&settings.azure_key_vault_certificate_name).await?;
		let need_new_certificate =
			certificate.map_or(true, |certificate| certificate.not_after < chrono::Utc::now() + chrono::Duration::days(30));
		need_new_certificate
	};
	if !need_new_certificate {
		logger.report_state(
			"azure/key_vault/certificate",
			(&settings.azure_key_vault_name, &settings.azure_key_vault_certificate_name),
			"does not need to be renewed",
		);
		return Ok("certificate does not need to be renewed");
	}

	let account_key = {
		let account_key = azure_key_vault_client.key_get(&settings.azure_key_vault_acme_account_key_name).await?;
		if let Some(account_key) = account_key {
			account_key
		}
		else {
			let (kty, crv) = settings.azure_key_vault_acme_account_key_type;
			let account_key =
				azure_key_vault_client.key_create(
					&settings.azure_key_vault_acme_account_key_name,
					kty,
					crv,
				).await?;
			account_key
		}
	};

	let mut acme_account = acme::Account::new(
		settings.acme_directory_url.clone(),
		&settings.acme_contact_url,
		&account_key,
		user_agent.clone(),
		logger,
	).await.context("could not initialize ACME API client")?;

	let domain_name = format!("*.{}", settings.top_level_domain_name);

	let mut acme_order = acme_account.place_order(&domain_name).await?;

	let certificates = {
		let azure_management_client = azure::management::Client::new(
			&azure_subscription_id,
			&settings.azure_resource_group_name,
			&azure_auth,
			user_agent,
			logger,
		).context("could not initialize Azure Management API client")?;

		let certificate = loop {
			match acme_order {
				acme::Order::Pending(pending) => {
					let () =
						azure_management_client.dns_txt_record_create(
							&settings.top_level_domain_name,
							"_acme-challenge",
							&pending.dns_txt_record_content,
						).await?;

					// Don't use `?` to fail immediately. Delete the TXT record first.
					let new_acme_order = acme_account.complete_authorization(pending).await;

					let () =
						azure_management_client.dns_txt_record_delete(
							&settings.top_level_domain_name,
							"_acme-challenge",
						).await?;

					acme_order = acme::Order::Ready(new_acme_order?);
				},

				acme::Order::Ready(ready) => {
					let csr =
						azure_key_vault_client.csr_create(
							&settings.azure_key_vault_certificate_name,
							&domain_name,
							settings.azure_key_vault_certificate_key_type,
						).await?;
					acme_order = acme::Order::Valid(acme_account.finalize_order(ready, csr).await?);
				},

				acme::Order::Valid(valid) =>
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
		azure_key_vault_client.certificate_merge(
			&settings.azure_key_vault_certificate_name,
			&certificates,
		).await?;

	logger.report_state(
		"azure/key_vault/certificate",
		(&settings.azure_key_vault_name, &settings.azure_key_vault_certificate_name),
		"renewed",
	);

	Ok("certificate has been renewed")
}

#[derive(serde::Deserialize)]
struct Settings<'a> {
	/// The directory URL of the ACME server
	#[serde(deserialize_with = "http_common::deserialize_http_uri")]
	acme_directory_url: http::Uri,

	/// The contact URL of the ACME account
	#[serde(borrow)]
	acme_contact_url: std::borrow::Cow<'a, str>,

	/// The name of the Azure resource group
	#[serde(borrow)]
	azure_resource_group_name: std::borrow::Cow<'a, str>,

	/// The name of the Azure KeyVault
	#[serde(borrow)]
	azure_key_vault_name: std::borrow::Cow<'a, str>,

	/// The name of the KeyVault secret that contains the ACME account key.
	///
	/// A new key will be generated and uploaded if this secret does not already exist.
	#[serde(borrow)]
	azure_key_vault_acme_account_key_name: std::borrow::Cow<'a, str>,

	/// The parameters used for the private key of the ACME account key if it needs to be created.
	#[serde(deserialize_with = "deserialize_key_vault_acme_account_key_type")]
	azure_key_vault_acme_account_key_type: (azure::key_vault::EcKty, azure::key_vault::EcCurve),

	/// The name of the certificate in the Azure KeyVault that contains the TLS certificate.
	///
	/// The new certificate will be uploaded here, and used for the custom domain.
	#[serde(borrow)]
	azure_key_vault_certificate_name: std::borrow::Cow<'a, str>,

	/// The parameters used for the private key of the new TLS certificate.
	#[serde(deserialize_with = "deserialize_key_vault_certificate_key_type")]
	azure_key_vault_certificate_key_type: azure::key_vault::CreateCsrKeyType,

	/// The domain name to request the TLS certificate for
	#[serde(borrow)]
	top_level_domain_name: std::borrow::Cow<'a, str>,
}

fn deserialize_key_vault_acme_account_key_type<'de, D>(deserializer: D) -> Result<(azure::key_vault::EcKty, azure::key_vault::EcCurve), D::Error>
where
	D: serde::Deserializer<'de>,
{
	struct Visitor;

	impl<'de> serde::de::Visitor<'de> for Visitor {
		type Value = (azure::key_vault::EcKty, azure::key_vault::EcCurve);

		fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
			f.write_str(r#"one of "#)?;
			f.write_str(r#""ec:p256", "ec-hsm:p256", "#)?;
			f.write_str(r#""ec:p384", "ec-hsm:p384", "#)?;
			f.write_str(r#""ec:p521", "ec-hsm:p521""#)?;
			Ok(())
		}

		fn visit_str<E>(self, s: &str) -> Result<Self::Value, E> where E: serde::de::Error {
			Ok(match s {
				"ec:p256" => (azure::key_vault::EcKty::Ec, azure::key_vault::EcCurve::P256),
				"ec-hsm:p256" => (azure::key_vault::EcKty::EcHsm, azure::key_vault::EcCurve::P256),
				"ec:p384" => (azure::key_vault::EcKty::Ec, azure::key_vault::EcCurve::P384),
				"ec-hsm:p384" => (azure::key_vault::EcKty::EcHsm, azure::key_vault::EcCurve::P384),
				"ec:p521" => (azure::key_vault::EcKty::Ec, azure::key_vault::EcCurve::P521),
				"ec-hsm:p521" => (azure::key_vault::EcKty::EcHsm, azure::key_vault::EcCurve::P521),

				s => return Err(serde::de::Error::invalid_value(serde::de::Unexpected::Str(s), &self)),
			})
		}
	}

	deserializer.deserialize_str(Visitor)
}

fn deserialize_key_vault_certificate_key_type<'de, D>(deserializer: D) -> Result<azure::key_vault::CreateCsrKeyType, D::Error>
where
	D: serde::Deserializer<'de>,
{
	struct Visitor;

	impl<'de> serde::de::Visitor<'de> for Visitor {
		type Value = azure::key_vault::CreateCsrKeyType;

		fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
			f.write_str(r#"one of "#)?;
			f.write_str(r#""rsa:2048", "rsa:2048:exportable", "rsa-hsm:2048", "#)?;
			f.write_str(r#""rsa:4096", "rsa:4096:exportable", "rsa-hsm:4096", "#)?;
			f.write_str(r#""ec:p256", "ec:p256:exportable", "ec-hsm:p256", "#)?;
			f.write_str(r#""ec:p384", "ec:p384:exportable", "ec-hsm:p384", "#)?;
			f.write_str(r#""ec:p521", "ec:p521:exportable", "ec-hsm:p521""#)?;
			Ok(())
		}

		fn visit_str<E>(self, s: &str) -> Result<Self::Value, E> where E: serde::de::Error {
			Ok(match s {
				"rsa:2048" => azure::key_vault::CreateCsrKeyType::Rsa { num_bits: 2048, exportable: false },
				"rsa:2048:exportable" => azure::key_vault::CreateCsrKeyType::Rsa { num_bits: 2048, exportable: true },
				"rsa-hsm:2048" => azure::key_vault::CreateCsrKeyType::RsaHsm { num_bits: 2048 },
				"rsa:4096" => azure::key_vault::CreateCsrKeyType::Rsa { num_bits: 4096, exportable: false },
				"rsa:4096:exportable" => azure::key_vault::CreateCsrKeyType::Rsa { num_bits: 4096, exportable: true },
				"rsa-hsm:4096" => azure::key_vault::CreateCsrKeyType::RsaHsm { num_bits: 4096 },
				"ec:p256" => azure::key_vault::CreateCsrKeyType::Ec { curve: azure::key_vault::EcCurve::P256, exportable: false },
				"ec:p256:exportable" => azure::key_vault::CreateCsrKeyType::Ec { curve: azure::key_vault::EcCurve::P256, exportable: true },
				"ec-hsm:p256" => azure::key_vault::CreateCsrKeyType::EcHsm { curve: azure::key_vault::EcCurve::P256 },
				"ec:p384" => azure::key_vault::CreateCsrKeyType::Ec { curve: azure::key_vault::EcCurve::P384, exportable: false },
				"ec:p384:exportable" => azure::key_vault::CreateCsrKeyType::Ec { curve: azure::key_vault::EcCurve::P384, exportable: true },
				"ec-hsm:p384" => azure::key_vault::CreateCsrKeyType::EcHsm { curve: azure::key_vault::EcCurve::P384 },
				"ec:p521" => azure::key_vault::CreateCsrKeyType::Ec { curve: azure::key_vault::EcCurve::P521, exportable: false },
				"ec:p521:exportable" => azure::key_vault::CreateCsrKeyType::Ec { curve: azure::key_vault::EcCurve::P521, exportable: true },
				"ec-hsm:p521" => azure::key_vault::CreateCsrKeyType::EcHsm { curve: azure::key_vault::EcCurve::P521 },

				s => return Err(serde::de::Error::invalid_value(serde::de::Unexpected::Str(s), &self)),
			})
		}
	}

	deserializer.deserialize_str(Visitor)
}
