impl<'a> super::Client<'a> {
	pub async fn cdn_custom_domain_secret_get(
		&self,
		cdn_profile_name: &str,
		cdn_endpoint_name: &str,
		cdn_custom_domain_name: &str,
	) -> anyhow::Result<Option<CustomDomainSecret<'static>>> {
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

		let secret =
			log2::report_operation(
				"azure/cdn/custom_domain/secret",
				(cdn_profile_name, cdn_endpoint_name, cdn_custom_domain_name),
				<log2::ScopedObjectOperation>::Get,
				async {
					let (url, authorization) =
						self.request_parameters(format_args!(
							"/providers/Microsoft.Cdn/profiles/{}/endpoints/{}/customDomains/{}?api-version=2019-12-31",
							cdn_profile_name,
							cdn_endpoint_name,
							cdn_custom_domain_name,
						)).await?;

					let Response { properties: CustomDomainProperties { custom_https_parameters } } =
						self.client.request(
							hyper::Method::GET,
							url,
							authorization,
							None::<&()>,
						).await?;
					let secret = custom_https_parameters.map(|custom_https_parameters| match custom_https_parameters {
						CustomDomainPropertiesCustomHttpsParameters::KeyVault { certificate_source_parameters, .. } =>
							CustomDomainSecret::KeyVault(certificate_source_parameters.secret.into_owned()),

						CustomDomainPropertiesCustomHttpsParameters::Cdn =>
							CustomDomainSecret::Cdn,
					});
					Ok::<_, anyhow::Error>(secret)
				},
			).await?;

		Ok(secret)
	}

	pub async fn cdn_custom_domain_secret_set(
		&self,
		cdn_profile_name: &str,
		cdn_endpoint_name: &str,
		cdn_custom_domain_name: &str,
		custom_domain_key_vault_secret: &CustomDomainKeyVaultSecret<'_>,
	) -> anyhow::Result<()> {
		#[derive(Debug)]
		enum Response {
			Ok,
			Accepted {
				location: hyper::Uri,
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

		let () =
			log2::report_operation(
				"azure/cdn/custom_domain/secret",
				(cdn_profile_name, cdn_endpoint_name, cdn_custom_domain_name),
				log2::ScopedObjectOperation::Create { value: format_args!("{:?}", custom_domain_key_vault_secret) },
				async {
					let (url, authorization) =
						self.request_parameters(format_args!(
							"/providers/Microsoft.Cdn/profiles/{}/endpoints/{}/customDomains/{}/enableCustomHttps?api-version=2019-12-31",
							cdn_profile_name,
							cdn_endpoint_name,
							cdn_custom_domain_name,
						)).await?;

					let mut response =
						self.client.request(
							hyper::Method::POST,
							url,
							authorization.clone(),
							Some(&CustomDomainPropertiesCustomHttpsParameters::KeyVault {
								certificate_source_parameters: CustomDomainPropertiesCustomHttpsParametersCertificateSourceParameters {
									delete_rule: "NoAction".into(),
									odata_type: "#Microsoft.Azure.Cdn.Models.KeyVaultCertificateSourceParameters".into(),
									update_rule: "NoAction".into(),
									secret: std::borrow::Cow::Borrowed(custom_domain_key_vault_secret),
								},
								protocol_type: "ServerNameIndication".into(),
							}),
						).await?;

					loop {
						match response {
							Response::Ok => break,

							Response::Accepted { location, retry_after } => {
								log2::report_message(format_args!("Waiting for {:?} before rechecking async operation...", retry_after));
								tokio::time::sleep(retry_after).await;

								log2::report_message(format_args!("Checking async operation {} ...", location));

								let new_response = self.client.request(
									hyper::Method::GET,
									location,
									authorization.clone(),
									None::<&()>,
								).await?;
								response = new_response;
							},
						}
					}

					Ok::<_, anyhow::Error>(())
				},
			).await?;

		Ok(())
	}
}

#[derive(Debug)]
pub enum CustomDomainSecret<'a> {
	KeyVault(CustomDomainKeyVaultSecret<'a>),
	Cdn,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CustomDomainKeyVaultSecret<'a> {
	#[serde(rename = "subscriptionId")]
	pub subscription_id: std::borrow::Cow<'a, str>,

	// This field doesn't serve any purpose because the KeyVault name is globally unique.
	// The CDN does in fact look up the KeyVault by its name, because specifying the wrong resource group
	// or even a non-existent resource group here still works. The only thing that doesn't work is
	// if this field is not serialized at all, or is the empty string, because the API fails the request.
	//
	// So it would be possible to not expose this as a struct field and instead just serialize a dummy value.
	// But for the sake of clarity and forward-compatibility, set it to the correct value anyway.
	#[serde(rename = "resourceGroupName")]
	pub resource_group: std::borrow::Cow<'a, str>,

	#[serde(rename = "vaultName")]
	pub key_vault_name: std::borrow::Cow<'a, str>,

	#[serde(rename = "secretName")]
	pub secret_name: std::borrow::Cow<'a, str>,

	#[serde(rename = "secretVersion")]
	pub secret_version: std::borrow::Cow<'a, str>,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(tag = "certificateSource")]
enum CustomDomainPropertiesCustomHttpsParameters<'a> {
	#[serde(rename = "AzureKeyVault")]
	KeyVault {
		#[serde(rename = "certificateSourceParameters")]
		certificate_source_parameters: CustomDomainPropertiesCustomHttpsParametersCertificateSourceParameters<'a>,

		#[serde(rename = "protocolType")]
		protocol_type: std::borrow::Cow<'a, str>,
	},

	Cdn,
}

#[derive(serde::Deserialize, serde::Serialize)]
struct CustomDomainPropertiesCustomHttpsParametersCertificateSourceParameters<'a> {
	#[serde(rename = "deleteRule")]
	delete_rule: std::borrow::Cow<'a, str>,

	#[serde(rename = "@odata.type")]
	odata_type: std::borrow::Cow<'a, str>,

	#[serde(rename = "updateRule")]
	update_rule: std::borrow::Cow<'a, str>,

	#[serde(flatten)]
	secret: std::borrow::Cow<'a, CustomDomainKeyVaultSecret<'a>>,
}
