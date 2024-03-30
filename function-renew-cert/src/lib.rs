use anyhow::Context;

pub async fn main(
	azure_subscription_id: &str,
	azure_auth: &azure::Auth,
	settings: &Settings<'_>,
	logger: &log2::Logger,
) -> anyhow::Result<()> {
	let user_agent: http_common::HeaderValue =
		concat!("github.com/Arnavion/acme-azure-function ", env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"))
		.parse().expect("hard-coded user agent is valid HeaderValue");

	let azure_key_vault_client = azure::key_vault::Client::new(
		&settings.azure_key_vault_name,
		azure_auth,
		user_agent.clone(),
		logger,
	).context("could not initialize Azure KeyVault API client")?;

	let mut acme_client = acme::Client::new(
		settings.acme_directory_url.0.clone(),
		user_agent.clone(),
		logger,
	).await.context("could not initialize ACME API client")?;

	{
		let now = time::OffsetDateTime::now_utc();

		let certificate = azure_key_vault_client.certificate_get(&settings.azure_key_vault_certificate_name).await?;
		if let Some(certificate) = certificate {
			let renewal_suggested_window_start =
				if let Some(ari_id) = certificate.ari_id {
					acme_client.renewal_suggested_window_start(&ari_id).await?
				}
				else {
					None
				};
			if let Some(renewal_suggested_window_start) = renewal_suggested_window_start {
				if renewal_suggested_window_start > now {
					logger.report_state(
						"azure/key_vault/certificate",
						(&settings.azure_key_vault_name, &settings.azure_key_vault_certificate_name),
						"does not need to be renewed",
					);
					return Ok(());
				}
			}
			else if certificate.not_after > now + time::Duration::days(30) {
				logger.report_state(
					"azure/key_vault/certificate",
					(&settings.azure_key_vault_name, &settings.azure_key_vault_certificate_name),
					"does not need to be renewed",
				);
				return Ok(());
			}
		}
	}

	let account_key = {
		let account_key = azure_key_vault_client.key_get(&settings.azure_key_vault_acme_account_key_name).await?;
		if let Some(account_key) = account_key {
			account_key
		}
		else {
			let (kty, crv) = settings.azure_key_vault_acme_account_key_type;
			azure_key_vault_client.key_create(
				&settings.azure_key_vault_acme_account_key_name,
				kty,
				crv,
			).await?
		}
	};

	let mut acme_account = acme_client.new_account(
		&settings.acme_contact_url,
		&account_key,
	).await.context("could not initialize ACME API client")?;

	let mut acme_order = acme_account.place_order(&settings.top_level_domain_name).await?;

	let certificates = {
		let azure_management_client = azure::management::Client::new(
			azure_subscription_id,
			&settings.azure_resource_group_name,
			azure_auth,
			user_agent,
			logger,
		).context("could not initialize Azure Management API client")?;

		let certificate = loop {
			match acme_order {
				acme::Order::Pending(pending) => {
					azure_management_client.dns_txt_record_create(
						&settings.top_level_domain_name,
						"_acme-challenge",
						pending.authorizations.iter().map(|authorization| &*authorization.dns_txt_record_content),
					).await?;

					// Don't use `?` to fail immediately. Delete the TXT record first.
					let new_acme_order = async {
						const MAX_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(500);

						let name_servers = azure_management_client.dns_zone_name_servers_get(&settings.top_level_domain_name).await?;
						let name_servers: futures_util::future::JoinAll<_> =
							name_servers.into_iter()
							.map(|name_server| tokio::net::lookup_host((name_server, 53)))
							.collect();
						let name_servers: Vec<_> =
							name_servers.await.into_iter()
							.flatten()
							.flatten()
							.flat_map(|socket_addr| [
								hickory_resolver::config::NameServerConfig::new(socket_addr, hickory_resolver::config::Protocol::Udp),
								hickory_resolver::config::NameServerConfig::new(socket_addr, hickory_resolver::config::Protocol::Tcp),
							])
							.collect();

						let name: hickory_resolver::Name = "_acme-challenge".parse().expect("hard-coded name is valid");
						let name = name.append_domain(&settings.top_level_domain_name.parse()?)?;

						let name_str = name.to_utf8();

						let resolver =
							hickory_resolver::AsyncResolver::tokio(
								hickory_resolver::config::ResolverConfig::from_parts(None, vec![], name_servers),
								Default::default(),
							);

						let mut retry_delay = std::time::Duration::from_millis(100);

						loop {
							let created = logger.report_operation("dns/lookup", &name_str, <log2::ScopedObjectOperation>::Get, async {
								resolver.clear_cache();
								match resolver.txt_lookup(name.clone()).await {
									Ok(_) => Ok(true),
									Err(err) if matches!(err.kind(), hickory_resolver::error::ResolveErrorKind::NoRecordsFound { .. }) => Ok(false),
									Err(err) => Err(anyhow::Error::from(err)),
								}
							}).await?;
							if created {
								break;
							}

							tokio::time::sleep(retry_delay).await;
							retry_delay = MAX_RETRY_DELAY.min(retry_delay * 2);
						}

						let new_acme_order = acme_account.complete_authorization(pending).await?;
						Ok::<_, anyhow::Error>(new_acme_order)
					};
					let new_acme_order = new_acme_order.await;

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
							&settings.top_level_domain_name,
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

	azure_key_vault_client.certificate_merge(
		&settings.azure_key_vault_certificate_name,
		&certificates,
	).await?;

	logger.report_state(
		"azure/key_vault/certificate",
		(&settings.azure_key_vault_name, &settings.azure_key_vault_certificate_name),
		"renewed",
	);

	_ =
		azure_key_vault_client.certificate_get(&settings.azure_key_vault_certificate_name).await?
		.context("newly-created certificate does not exist")?;

	Ok(())
}

#[derive(serde::Deserialize)]
pub struct Settings<'a> {
	/// The directory URL of the ACME server
	acme_directory_url: http_common::DeserializableUri,

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
	azure_key_vault_acme_account_key_type: (azure::key_vault::EcKty, acme::EcCurve),

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

fn deserialize_key_vault_acme_account_key_type<'de, D>(deserializer: D) -> Result<(azure::key_vault::EcKty, acme::EcCurve), D::Error>
where
	D: serde::Deserializer<'de>,
{
	struct Visitor;

	impl<'de> serde::de::Visitor<'de> for Visitor {
		type Value = (azure::key_vault::EcKty, acme::EcCurve);

		fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
			f.write_str("one of ")?;
			f.write_str(r#""ec:p256", "ec-hsm:p256", "#)?;
			f.write_str(r#""ec:p384", "ec-hsm:p384", "#)?;
			f.write_str(r#""ec:p521", "ec-hsm:p521""#)?;
			Ok(())
		}

		fn visit_str<E>(self, s: &str) -> Result<Self::Value, E> where E: serde::de::Error {
			Ok(match s {
				"ec:p256" => (azure::key_vault::EcKty::Ec, acme::EcCurve::P256),
				"ec-hsm:p256" => (azure::key_vault::EcKty::EcHsm, acme::EcCurve::P256),
				"ec:p384" => (azure::key_vault::EcKty::Ec, acme::EcCurve::P384),
				"ec-hsm:p384" => (azure::key_vault::EcKty::EcHsm, acme::EcCurve::P384),
				"ec:p521" => (azure::key_vault::EcKty::Ec, acme::EcCurve::P521),
				"ec-hsm:p521" => (azure::key_vault::EcKty::EcHsm, acme::EcCurve::P521),

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
			f.write_str("one of ")?;
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
				"ec:p256" => azure::key_vault::CreateCsrKeyType::Ec { curve: acme::EcCurve::P256, exportable: false },
				"ec:p256:exportable" => azure::key_vault::CreateCsrKeyType::Ec { curve: acme::EcCurve::P256, exportable: true },
				"ec-hsm:p256" => azure::key_vault::CreateCsrKeyType::EcHsm { curve: acme::EcCurve::P256 },
				"ec:p384" => azure::key_vault::CreateCsrKeyType::Ec { curve: acme::EcCurve::P384, exportable: false },
				"ec:p384:exportable" => azure::key_vault::CreateCsrKeyType::Ec { curve: acme::EcCurve::P384, exportable: true },
				"ec-hsm:p384" => azure::key_vault::CreateCsrKeyType::EcHsm { curve: acme::EcCurve::P384 },
				"ec:p521" => azure::key_vault::CreateCsrKeyType::Ec { curve: acme::EcCurve::P521, exportable: false },
				"ec:p521:exportable" => azure::key_vault::CreateCsrKeyType::Ec { curve: acme::EcCurve::P521, exportable: true },
				"ec-hsm:p521" => azure::key_vault::CreateCsrKeyType::EcHsm { curve: acme::EcCurve::P521 },

				s => return Err(serde::de::Error::invalid_value(serde::de::Unexpected::Str(s), &self)),
			})
		}
	}

	deserializer.deserialize_str(Visitor)
}
