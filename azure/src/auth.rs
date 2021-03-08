use anyhow::Context;

pub enum Auth {
	ManagedIdentity {
		endpoint: String,
		secret: hyper::header::HeaderValue,
	},

	#[cfg(debug_assertions)]
	ServicePrincipal {
		client_id: String,
		client_secret: String,
		tenant_id: String,
	},
}

impl Auth {
	pub(crate) async fn get_authorization(&self, client: &http_common::Client, resource: &str) -> anyhow::Result<hyper::header::HeaderValue> {
		// TODO: Workaround for https://github.com/rust-lang/rust/issues/55779 when running
		// `cargo build --manifest-path ./azure/Cargo.toml --features dns`
		#[allow(unused_extern_crates)]
		extern crate serde;

		#[derive(serde::Deserialize)]
		struct Response {
			access_token: String,
			token_type: String,
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

		let log2::Secret(authorization) = log2::report_operation("azure/authorization", resource, <log2::ScopedObjectOperation>::Get, async {
			let req = match self {
				Auth::ManagedIdentity { endpoint, secret } => {
					static SECRET: once_cell2::race::LazyBox<hyper::header::HeaderName> =
						once_cell2::race::LazyBox::new(|| hyper::header::HeaderName::from_static("secret"));

					let mut req = hyper::Request::new(Default::default());
					*req.method_mut() = hyper::Method::GET;
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
					let mut req = hyper::Request::new(body.into());
					*req.method_mut() = hyper::Method::POST;
					*req.uri_mut() =
						std::convert::TryInto::try_into(format!("https://login.microsoftonline.com/{}/oauth2/token", tenant_id))
						.context("could not construct authorization request URI")?;
					req
				},
			};

			let Response { access_token, token_type } =
				client.request_inner(req).await.context("could not get authorization")?;

			let header_value =
				std::convert::TryInto::try_into(format!("{} {}", token_type, access_token))
				.context("could not parse token as HeaderValue")?;
			Ok::<_, anyhow::Error>(log2::Secret(header_value))
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
