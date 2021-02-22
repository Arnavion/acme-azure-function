impl<'a> crate::Account<'a> {
	pub async fn cdn_custom_domain_secret_get(
		&self,
		cdn_profile_name: &str,
		cdn_endpoint_name: &str,
		cdn_custom_domain_name: &str,
	) -> anyhow::Result<Option<CustomDomainSecret>> {
		#[derive(serde::Deserialize)]
		struct Response {
			properties: CustomDomainProperties,
		}

		#[derive(serde::Deserialize)]
		struct CustomDomainProperties {
			#[serde(rename = "customHttpsParameters")]
			custom_https_parameters: Option<CustomDomainPropertiesCustomHttpsParameters<'static>>,
		}

		impl http_common::FromResponse for Response {
			fn from_response(
				status: hyper::StatusCode,
				body: Option<(&hyper::header::HeaderValue, &mut impl std::io::Read)>,
				_headers: hyper::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				Ok(match (status, body) {
					(hyper::StatusCode::OK, Some((content_type, body))) if http_common::is_json(content_type) =>
						Some(serde_json::from_reader(body)?),
					_ => None,
				})
			}
		}

		eprintln!("Getting CDN custom domain {}/{}/{} secret version", cdn_profile_name, cdn_endpoint_name, cdn_custom_domain_name);

		let management_request_parameters =
			self.management_request_parameters(format_args!(
				"/providers/Microsoft.Cdn/profiles/{}/endpoints/{}/customDomains/{}?api-version=2018-04-02",
				cdn_profile_name,
				cdn_endpoint_name,
				cdn_custom_domain_name,
			));
		let (url, authorization) = management_request_parameters.await?;

		let response: Response =
			self.client.request(
				hyper::Method::GET,
				&url,
				authorization,
				None::<&()>,
			).await?;
		let secret =
			response.properties.custom_https_parameters
			.map(|custom_https_parameters| CustomDomainSecret {
				name: custom_https_parameters.certificate_source_parameters.secret_name.into_owned(),
				version: custom_https_parameters.certificate_source_parameters.secret_version.into_owned(),
			});

		if let Some(secret) = &secret {
			eprintln!("CDN custom domain {}/{}/{} has secret {:?}", cdn_profile_name, cdn_endpoint_name, cdn_custom_domain_name, secret);
		}
		else {
			eprintln!("CDN custom domain {}/{}/{} does not have HTTPS enabled", cdn_profile_name, cdn_endpoint_name, cdn_custom_domain_name);
		}

		Ok(secret)
	}

	pub async fn cdn_custom_domain_secret_set(
		&self,
		cdn_profile_name: &str,
		cdn_endpoint_name: &str,
		cdn_custom_domain_name: &str,
		key_vault_name: &str,
		key_vault_secret_name: &str,
		key_vault_secret_version: &str,
	) -> anyhow::Result<()> {
		#[derive(Debug)]
		enum Response {
			Ok,
			Accepted {
				location: String,
				retry_after: std::time::Duration,
			},
		}

		impl http_common::FromResponse for Response {
			fn from_response(
				status: hyper::StatusCode,
				_body: Option<(&hyper::header::HeaderValue, &mut impl std::io::Read)>,
				headers: hyper::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				Ok(match status {
					hyper::StatusCode::OK => Some(Response::Ok),
					hyper::StatusCode::ACCEPTED => {
						let location = http_common::get_location(&headers)?;
						let retry_after = http_common::get_retry_after(&headers, std::time::Duration::from_secs(1), std::time::Duration::from_secs(30))?;
						Some(Response::Accepted {
							location,
							retry_after,
						})
					},
					_ => None,
				})
			}
		}

		eprintln!(
			"Setting CDN custom domain {}/{}/{} secret to {}/{}/{} ...",
			cdn_profile_name,
			cdn_endpoint_name,
			cdn_custom_domain_name,
			key_vault_name,
			key_vault_secret_name,
			key_vault_secret_version,
		);

		let management_request_parameters =
			self.management_request_parameters(format_args!(
				"/providers/Microsoft.Cdn/profiles/{}/endpoints/{}/customDomains/{}/enableCustomHttps?api-version=2018-04-02",
				cdn_profile_name,
				cdn_endpoint_name,
				cdn_custom_domain_name,
			));
		let (url, authorization) = management_request_parameters.await?;

		let mut response =
			self.client.request(
				hyper::Method::POST,
				&url,
				authorization.clone(),
				Some(&CustomDomainPropertiesCustomHttpsParameters {
					certificate_source: "AzureKeyVault".into(),
					certificate_source_parameters: CustomDomainPropertiesCustomHttpsParametersCertificateSourceParameters {
						delete_rule: "NoAction".into(),
						key_vault_name: key_vault_name.into(),
						odata_type: "#Microsoft.Azure.Cdn.Models.KeyVaultCertificateSourceParameters".into(),
						resource_group: self.resource_group_name.into(),
						secret_name: key_vault_secret_name.into(),
						secret_version: key_vault_secret_version.into(),
						subscription_id: self.subscription_id.into(),
						update_rule: "NoAction".into(),
					},
					protocol_type: "ServerNameIndication".into(),
				}),
			).await?;

		loop {
			match response {
				Response::Ok => break,

				Response::Accepted { location, retry_after } => {
					eprintln!("Waiting for {:?} before rechecking async operation...", retry_after);
					tokio::time::sleep(retry_after).await;

					eprintln!("Checking async operation {} ...", location);

					let new_response = self.client.request(
						hyper::Method::GET,
						&location,
						authorization.clone(),
						None::<&()>,
					).await?;
					response = new_response;
				},
			}
		}

		eprintln!(
			"Set CDN custom domain {}/{}/{} secret to {}/{}/{}",
			cdn_profile_name,
			cdn_endpoint_name,
			cdn_custom_domain_name,
			key_vault_name,
			key_vault_secret_name,
			key_vault_secret_version,
		);

		Ok(())
	}
}

#[derive(Debug)]
pub struct CustomDomainSecret {
	pub name: String,
	pub version: String,
}

#[derive(serde::Deserialize, serde::Serialize)]
struct CustomDomainPropertiesCustomHttpsParameters<'a> {
	#[serde(rename = "certificateSource")]
	certificate_source: std::borrow::Cow<'a, str>,

	#[serde(rename = "certificateSourceParameters")]
	certificate_source_parameters: CustomDomainPropertiesCustomHttpsParametersCertificateSourceParameters<'a>,

	#[serde(rename = "protocolType")]
	protocol_type: std::borrow::Cow<'a, str>,
}

#[derive(serde::Deserialize, serde::Serialize)]
struct CustomDomainPropertiesCustomHttpsParametersCertificateSourceParameters<'a> {
	#[serde(rename = "deleteRule")]
	delete_rule: std::borrow::Cow<'a, str>,

	#[serde(rename = "vaultName")]
	key_vault_name: std::borrow::Cow<'a, str>,

	#[serde(rename = "@odata.type")]
	odata_type: std::borrow::Cow<'a, str>,

	#[serde(rename = "resourceGroupName")]
	resource_group: std::borrow::Cow<'a, str>,

	#[serde(rename = "secretName")]
	secret_name: std::borrow::Cow<'a, str>,

	#[serde(rename = "secretVersion")]
	secret_version: std::borrow::Cow<'a, str>,

	#[serde(rename = "subscriptionId")]
	subscription_id: std::borrow::Cow<'a, str>,

	#[serde(rename = "updateRule")]
	update_rule: std::borrow::Cow<'a, str>,
}
