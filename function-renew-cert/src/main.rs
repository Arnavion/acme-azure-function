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

async fn renew_cert_main(settings: std::rc::Rc<Settings>) -> anyhow::Result<()> {
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

	let need_new_certificate = {
		let certificate = azure_account.key_vault_certificate_get(&settings.azure_key_vault_name, &settings.azure_key_vault_certificate_name).await?;
		let need_new_certificate =
			certificate.map_or(true, |certificate| certificate.not_after < chrono::Utc::now() + chrono::Duration::days(30));
		need_new_certificate
	};
	if !need_new_certificate {
		log2::report_state(
			"azure/key_vault/certificate",
			(&settings.azure_key_vault_name, &settings.azure_key_vault_certificate_name),
			"does not need to be renewed",
		);
		return Ok(());
	}

	let account_key = {
		let account_key = azure_account.key_vault_key_get(&settings.azure_key_vault_name, &settings.azure_key_vault_acme_account_key_name).await?;
		if let Some(account_key) = account_key {
			account_key
		}
		else {
			let (kty, crv) = settings.azure_key_vault_acme_account_key_type;
			let account_key =
				azure_account.key_vault_key_create(
					&settings.azure_key_vault_name,
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
		concat!("github.com/Arnavion/acme-azure-function ", env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION")),
	).await.context("could not initialize ACME API client")?;

	let domain_name = format!("*.{}", settings.top_level_domain_name);

	let mut acme_order = acme_account.place_order(&domain_name).await?;

	let certificates = {
		let certificate = loop {
			match acme_order {
				acme::Order::Pending(pending) => {
					let () =
						azure_account.dns_txt_record_create(
							&settings.top_level_domain_name,
							"_acme-challenge",
							&pending.dns_txt_record_content,
						).await?;

					// Don't use `?` to fail immediately. Delete the TXT record first.
					let new_acme_order = acme_account.complete_authorization(pending).await;

					let () =
						azure_account.dns_txt_record_delete(
							&settings.top_level_domain_name,
							"_acme-challenge",
						).await?;

					acme_order = acme::Order::Ready(new_acme_order?);
				},

				acme::Order::Ready(ready) => {
					let csr =
						azure_account.key_vault_csr_create(
							&settings.azure_key_vault_name,
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
		azure_account.key_vault_certificate_merge(
			&settings.azure_key_vault_name,
			&settings.azure_key_vault_certificate_name,
			&certificates,
		).await?;

	log2::report_state(
		"azure/key_vault/certificate",
		(&settings.azure_key_vault_name, &settings.azure_key_vault_certificate_name),
		"renewed",
	);

	Ok(())
}

#[derive(serde::Deserialize)]
struct Settings {
	/// The Azure subscription ID
	azure_subscription_id: String,

	/// The directory URL of the ACME server
	#[serde(deserialize_with = "http_common::deserialize_hyper_uri")]
	acme_directory_url: hyper::Uri,

	/// The contact URL of the ACME account
	acme_contact_url: String,

	/// The name of the Azure resource group
	azure_resource_group_name: String,

	/// The name of the Azure KeyVault
	azure_key_vault_name: String,

	/// The name of the KeyVault secret that contains the ACME account key.
	///
	/// A new key will be generated and uploaded if this secret does not already exist.
	azure_key_vault_acme_account_key_name: String,

	/// The parameters used for the private key of the ACME account key if it needs to be created.
	#[serde(deserialize_with = "deserialize_key_vault_acme_account_key_type")]
	azure_key_vault_acme_account_key_type: (azure::EcKty, azure::EcCurve),

	// /// The name of the KeyVault secret that contains the ACME account key.
	// ///
	// /// A new key will be generated and uploaded if this secret does not already exist.
	// azure_key_vault_acme_account_key_name: String,

	/// The name of the certificate in the Azure KeyVault that contains the TLS certificate.
	///
	/// The new certificate will be uploaded here, and used for the custom domain.
	azure_key_vault_certificate_name: String,

	/// The parameters used for the private key of the new TLS certificate.
	#[serde(deserialize_with = "deserialize_key_vault_certificate_key_type")]
	azure_key_vault_certificate_key_type: azure::KeyVaultCreateCsrKeyType,

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

fn deserialize_key_vault_acme_account_key_type<'de, D>(deserializer: D) -> Result<(azure::EcKty, azure::EcCurve), D::Error>
where
	D: serde::Deserializer<'de>,
{
	struct Visitor;

	impl<'de> serde::de::Visitor<'de> for Visitor {
		type Value = (azure::EcKty, azure::EcCurve);

		fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
			f.write_str(r#"one of "#)?;
			f.write_str(r#""ec:p256", "ec-hsm:p256", "#)?;
			f.write_str(r#""ec:p384", "ec-hsm:p384", "#)?;
			f.write_str(r#""ec:p521", "ec-hsm:p521""#)?;
			Ok(())
		}

		fn visit_str<E>(self, s: &str) -> Result<Self::Value, E> where E: serde::de::Error {
			Ok(match s {
				"ec:p256" => (azure::EcKty::Ec, azure::EcCurve::P256),
				"ec-hsm:p256" => (azure::EcKty::EcHsm, azure::EcCurve::P256),
				"ec:p384" => (azure::EcKty::Ec, azure::EcCurve::P384),
				"ec-hsm:p384" => (azure::EcKty::EcHsm, azure::EcCurve::P384),
				"ec:p521" => (azure::EcKty::Ec, azure::EcCurve::P521),
				"ec-hsm:p521" => (azure::EcKty::EcHsm, azure::EcCurve::P521),

				s => return Err(serde::de::Error::invalid_value(serde::de::Unexpected::Str(s), &self)),
			})
		}
	}

	deserializer.deserialize_str(Visitor)
}

fn deserialize_key_vault_certificate_key_type<'de, D>(deserializer: D) -> Result<azure::KeyVaultCreateCsrKeyType, D::Error>
where
	D: serde::Deserializer<'de>,
{
	struct Visitor;

	impl<'de> serde::de::Visitor<'de> for Visitor {
		type Value = azure::KeyVaultCreateCsrKeyType;

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
				"rsa:2048" => azure::KeyVaultCreateCsrKeyType::Rsa { num_bits: 2048, exportable: false },
				"rsa:2048:exportable" => azure::KeyVaultCreateCsrKeyType::Rsa { num_bits: 2048, exportable: true },
				"rsa-hsm:2048" => azure::KeyVaultCreateCsrKeyType::RsaHsm { num_bits: 2048 },
				"rsa:4096" => azure::KeyVaultCreateCsrKeyType::Rsa { num_bits: 4096, exportable: false },
				"rsa:4096:exportable" => azure::KeyVaultCreateCsrKeyType::Rsa { num_bits: 4096, exportable: true },
				"rsa-hsm:4096" => azure::KeyVaultCreateCsrKeyType::RsaHsm { num_bits: 4096 },
				"ec:p256" => azure::KeyVaultCreateCsrKeyType::Ec { curve: azure::EcCurve::P256, exportable: false },
				"ec:p256:exportable" => azure::KeyVaultCreateCsrKeyType::Ec { curve: azure::EcCurve::P256, exportable: true },
				"ec-hsm:p256" => azure::KeyVaultCreateCsrKeyType::EcHsm { curve: azure::EcCurve::P256 },
				"ec:p384" => azure::KeyVaultCreateCsrKeyType::Ec { curve: azure::EcCurve::P384, exportable: false },
				"ec:p384:exportable" => azure::KeyVaultCreateCsrKeyType::Ec { curve: azure::EcCurve::P384, exportable: true },
				"ec-hsm:p384" => azure::KeyVaultCreateCsrKeyType::EcHsm { curve: azure::EcCurve::P384 },
				"ec:p521" => azure::KeyVaultCreateCsrKeyType::Ec { curve: azure::EcCurve::P521, exportable: false },
				"ec:p521:exportable" => azure::KeyVaultCreateCsrKeyType::Ec { curve: azure::EcCurve::P521, exportable: true },
				"ec-hsm:p521" => azure::KeyVaultCreateCsrKeyType::EcHsm { curve: azure::EcCurve::P521 },

				s => return Err(serde::de::Error::invalid_value(serde::de::Unexpected::Str(s), &self)),
			})
		}
	}

	deserializer.deserialize_str(Visitor)
}
