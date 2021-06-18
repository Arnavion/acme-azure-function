use anyhow::Context;

pub enum Auth {
	ManagedIdentity {
		endpoint: String,
		secret: http::HeaderValue,
	},

	#[cfg(debug_assertions)]
	ServicePrincipal {
		client_id: String,
		client_secret: String,
		tenant_id: String,
	},
}

impl Auth {
	pub(crate) async fn get_authorization(
		&self,
		client: &http_common::Client,
		resource: &str,
		logger: &log2::Logger,
	) -> anyhow::Result<http::HeaderValue> {
		struct Response(http::HeaderValue);

		impl http_common::FromResponse for Response {
			fn from_response(
				status: http::StatusCode,
				body: Option<(&http::HeaderValue, &mut http_common::Body<impl std::io::Read>)>,
				_headers: http::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				#[derive(serde::Deserialize)]
				struct ResponseInner<'a> {
					#[serde(borrow)]
					access_token: std::borrow::Cow<'a, str>,
					#[serde(borrow)]
					token_type: std::borrow::Cow<'a, str>,
				}

				Ok(match (status, body) {
					(http::StatusCode::OK, Some((content_type, body))) if http_common::is_json(content_type) => {
						let ResponseInner { access_token, token_type } = body.as_json()?;
						let header_value =
							std::convert::TryInto::try_into(format!("{} {}", token_type, access_token))
							.context("could not parse token as HeaderValue")?;
						Some(Response(header_value))
					},
					_ => None,
				})
			}
		}

		let log2::Secret(authorization) = logger.report_operation("azure/authorization", resource, <log2::ScopedObjectOperation>::Get, async {
			let req = match self {
				Auth::ManagedIdentity { endpoint, secret } => {
					static SECRET: once_cell2::race::LazyBox<http::header::HeaderName> =
						once_cell2::race::LazyBox::new(|| http::header::HeaderName::from_static("secret"));

					let mut req = http::Request::new(Default::default());
					*req.method_mut() = http::Method::GET;
					*req.uri_mut() =
						std::convert::TryInto::try_into(format!("{}?resource={}&api-version=2017-09-01", endpoint, resource))
						.context("could not construct authorization request URI")?;
					req.headers_mut().insert(SECRET.clone(), secret.clone());
					req
				},

				#[cfg(debug_assertions)]
				Auth::ServicePrincipal { client_id, client_secret, tenant_id } => {
					let body =
						form_urlencoded::Serializer::new(String::new())
						.append_pair("grant_type", "client_credentials")
						.append_pair("client_id", client_id)
						.append_pair("client_secret", client_secret)
						.append_pair("resource", resource)
						.finish();
					let mut req = http::Request::new(body.into());
					*req.method_mut() = http::Method::POST;
					*req.uri_mut() =
						std::convert::TryInto::try_into(format!("https://login.microsoftonline.com/{}/oauth2/token", tenant_id))
						.context("could not construct authorization request URI")?;
					req
				},
			};

			let Response(header_value) =
				client.request(req).await.context("could not get authorization")?;
			Ok(log2::Secret(header_value))
		}).await?;
		Ok(authorization)
	}
}

impl<'de> serde::Deserialize<'de> for Auth {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> where D: serde::Deserializer<'de> {
		if let (Ok(endpoint), Ok(secret)) = (std::env::var("MSI_ENDPOINT"), std::env::var("MSI_SECRET")) {
			let _ = deserializer;
			let secret =
				std::convert::TryInto::try_into(secret)
				.map_err(|err| serde::de::Error::custom(format!("could not parse MSI_SECRET as HeaderValue: {}", err)))?;
			return Ok(Auth::ManagedIdentity {
				endpoint,
				secret,
			});
		}

		#[cfg(debug_assertions)]
		{
			#[derive(serde::Deserialize)]
			struct AuthInner {
				/// The application ID of the service principal that this Function should use to access Azure resources.
				///
				/// Only needed for local testing; the final released Function should be set to use the Function app MSI.
				azure_client_id: String,

				/// The password of the service principal that this Function should use to access Azure resources.
				///
				/// Only needed for local testing; the final released Function should be set to use the Function app MSI.
				azure_client_secret: String,

				/// The tenant ID of the service principal that this Function should use to access Azure resources.
				///
				/// Only needed for local testing; the final released Function should be set to use the Function app MSI.
				azure_tenant_id: String,
			}

			let AuthInner { azure_client_id, azure_client_secret, azure_tenant_id } = serde::Deserialize::deserialize(deserializer)?;
			Ok(Auth::ServicePrincipal {
				client_id: azure_client_id,
				client_secret: azure_client_secret,
				tenant_id: azure_tenant_id,
			})
		}

		#[cfg(not(debug_assertions))]
		Err(serde::de::Error::custom("did not find MSI_ENDPOINT and MSI_SECRET env vars"))
	}
}
