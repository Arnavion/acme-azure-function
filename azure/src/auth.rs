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
	pub fn from_env(
		#[cfg_attr(not(debug_assertions), allow(unused_variables))]
		client_id: Option<String>,
		#[cfg_attr(not(debug_assertions), allow(unused_variables))]
		client_secret: Option<String>,
		#[cfg_attr(not(debug_assertions), allow(unused_variables))]
		tenant_id: Option<String>,
	) -> anyhow::Result<Self> {
		if let (Ok(endpoint), Ok(secret)) = (std::env::var("MSI_ENDPOINT"), std::env::var("MSI_SECRET")) {
			let secret = std::convert::TryInto::try_into(&secret).context("could not parse MSI_SECRET as HeaderValue")?;
			return Ok(Auth::ManagedIdentity { endpoint, secret });
		}

		#[cfg(debug_assertions)]
		if let (Some(client_id), Some(client_secret), Some(tenant_id)) = (client_id, client_secret, tenant_id) {
			return Ok(Auth::ServicePrincipal { client_id, client_secret, tenant_id });
		}

		Err(anyhow::anyhow!("found neither MSI_ENDPOINT+MSI_SECRET nor client_id+client_secret+tenant_id"))
	}

	pub(crate) async fn get_authorization(&self, client: &http_common::Client, resource: &str) -> anyhow::Result<hyper::header::HeaderValue> {
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
						format!("{}?resource={}&api-version=2017-09-01", endpoint, resource)
						.parse().context("could not construct authorization request URI")?;
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
						format!("https://login.microsoftonline.com/{}/oauth2/token", tenant_id)
						.parse().context("could not construct authorization request URI")?;
					req
				},
			};

			let Response { access_token, token_type } =
				client.request_inner(req).await.context("could not get authorization")?;

			let header_value = format!("{} {}", token_type, access_token);
			let header_value: hyper::header::HeaderValue = header_value.parse().context("could not parse token as HeaderValue")?;
			Ok::<_, anyhow::Error>(log2::Secret(header_value))
		}).await?;
		Ok(authorization)
	}
}
