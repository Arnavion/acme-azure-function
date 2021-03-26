impl<'a> super::Client<'a> {
	pub async fn cdn_custom_domain_secret_get(
		&self,
		cdn_profile_name: &str,
		cdn_endpoint_name: &str,
		cdn_custom_domain_name: &str,
	) -> anyhow::Result<Option<CustomDomainSecret<'static>>> {
		struct Response(Option<CustomDomainSecret<'static>>);

		impl http_common::FromResponse for Response {
			fn from_response(
				status: http::StatusCode,
				body: Option<(&http::HeaderValue, &mut http_common::Body<impl std::io::Read>)>,
				_headers: http::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				#[derive(serde::Deserialize)]
				struct ResponseInner<'a> {
					#[serde(borrow)]
					properties: CustomDomainProperties<'a>,
				}

				#[derive(serde::Deserialize)]
				struct CustomDomainProperties<'a> {
					#[serde(borrow, rename = "customHttpsParameters")]
					custom_https_parameters: Option<CustomDomainPropertiesCustomHttpsParameters<'a>>,
				}

				Ok(match (status, body) {
					(http::StatusCode::OK, Some((content_type, body))) if http_common::is_json(content_type) => {
						let ResponseInner { properties: CustomDomainProperties { custom_https_parameters } } = body.as_json()?;
						let secret = custom_https_parameters.map(|custom_https_parameters| match custom_https_parameters {
							CustomDomainPropertiesCustomHttpsParameters::KeyVault {
								certificate_source_parameters: CustomDomainPropertiesCustomHttpsParametersCertificateSourceParameters {
									secret,
									..
								},
								..
							} => {
								let CustomDomainKeyVaultSecret {
									subscription_id,
									resource_group,
									key_vault_name,
									secret_name,
									secret_version,
								} = secret.into_owned();
								let secret = CustomDomainKeyVaultSecret {
									subscription_id: subscription_id.into_owned().into(),
									resource_group: resource_group.into_owned().into(),
									key_vault_name: key_vault_name.into_owned().into(),
									secret_name: secret_name.into_owned().into(),
									secret_version: secret_version.into_owned().into(),
								};
								CustomDomainSecret::KeyVault(secret)
							},

							CustomDomainPropertiesCustomHttpsParameters::Cdn =>
								CustomDomainSecret::Cdn,
						});
						Some(Response(secret))
					},
					_ => None,
				})
			}
		}

		let secret =
			self.logger.report_operation(
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

					let Response(secret) =
						self.client.request(
							http::Method::GET,
							url,
							authorization,
							None::<&()>,
						).await?;
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
				location: http::Uri,
				retry_after: std::time::Duration,
			},
		}

		impl http_common::FromResponse for Response {
			fn from_response(
				status: http::StatusCode,
				_body: Option<(&http::HeaderValue, &mut http_common::Body<impl std::io::Read>)>,
				headers: http::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				Ok(match status {
					http::StatusCode::OK => Some(Response::Ok),
					http::StatusCode::ACCEPTED => {
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
			self.logger.report_operation(
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
							http::Method::POST,
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
								self.logger.report_message(format_args!("Waiting for {:?} before rechecking async operation...", retry_after));
								tokio::time::sleep(retry_after).await;

								self.logger.report_message(format_args!("Checking async operation {} ...", location));

								let new_response = self.client.request(
									http::Method::GET,
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
	#[serde(borrow, rename = "subscriptionId")]
	pub subscription_id: std::borrow::Cow<'a, str>,

	// This field doesn't serve any purpose because the KeyVault name is globally unique.
	// The CDN does in fact look up the KeyVault by its name, because specifying the wrong resource group
	// or even a non-existent resource group here still works. The only thing that doesn't work is
	// if this field is not serialized at all, or is the empty string, because the API fails the request.
	//
	// So it would be possible to not expose this as a struct field and instead just serialize a dummy value.
	// But for the sake of clarity and forward-compatibility, set it to the correct value anyway.
	#[serde(borrow, rename = "resourceGroupName")]
	pub resource_group: std::borrow::Cow<'a, str>,

	#[serde(borrow, rename = "vaultName")]
	pub key_vault_name: std::borrow::Cow<'a, str>,

	#[serde(borrow, rename = "secretName")]
	pub secret_name: std::borrow::Cow<'a, str>,

	#[serde(borrow, rename = "secretVersion")]
	pub secret_version: std::borrow::Cow<'a, str>,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(tag = "certificateSource")]
enum CustomDomainPropertiesCustomHttpsParameters<'a> {
	#[serde(rename = "AzureKeyVault")]
	KeyVault {
		#[serde(borrow, rename = "certificateSourceParameters")]
		certificate_source_parameters: CustomDomainPropertiesCustomHttpsParametersCertificateSourceParameters<'a>,

		#[serde(borrow, rename = "protocolType")]
		protocol_type: std::borrow::Cow<'a, str>,
	},

	Cdn,
}

#[derive(serde::Deserialize, serde::Serialize)]
struct CustomDomainPropertiesCustomHttpsParametersCertificateSourceParameters<'a> {
	#[serde(borrow, rename = "deleteRule")]
	delete_rule: std::borrow::Cow<'a, str>,

	#[serde(borrow, rename = "@odata.type")]
	odata_type: std::borrow::Cow<'a, str>,

	#[serde(borrow, rename = "updateRule")]
	update_rule: std::borrow::Cow<'a, str>,

	#[serde(borrow, flatten)]
	secret: std::borrow::Cow<'a, CustomDomainKeyVaultSecret<'a>>,
}
