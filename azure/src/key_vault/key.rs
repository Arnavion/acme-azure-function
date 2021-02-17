impl<'a> crate::Account<'a> {
	pub async fn key_vault_key_create(
		&mut self,
		key_vault_name: &str,
		key_name: &str,
	) -> anyhow::Result<Key> {
		#[derive(serde::Serialize)]
		struct Request<'a> {
			crv: &'a str,
			kty: &'a str,
			key_ops: &'a [&'a str],
		}

		eprintln!("Creating key {}/{} ...", key_vault_name, key_name);

		let (url, authorization) =
			self.key_vault_request_parameters(
				key_vault_name,
				&format!("/keys/{}/create?api-version=7.1", key_name),
			).await?;

		let CreateOrGetKeyResponse { key } =
			self.client.request(
				hyper::Method::POST,
				&url,
				authorization,
				Some(&Request {
					crv: "P-384",
					kty: "EC",
					key_ops: &["sign", "verify"],
				}),
			).await?;

		eprintln!("Created key {}/{}: {:?}", key_vault_name, key_name, key);

		Ok(key)
	}

	pub async fn key_vault_key_get(
		&mut self,
		key_vault_name: &str,
		key_name: &str,
	) -> anyhow::Result<Option<Key>> {
		struct Response(Option<CreateOrGetKeyResponse>);

		impl http_common::FromResponse for Response {
			fn from_response(
				status: hyper::StatusCode,
				body: Option<(&hyper::header::HeaderValue, &mut impl std::io::Read)>,
				_headers: hyper::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				Ok(match (status, body) {
					(hyper::StatusCode::OK, Some((content_type, body))) if http_common::is_json(content_type) =>
						Some(Response(Some(serde_json::from_reader(body)?))),
					(hyper::StatusCode::NOT_FOUND, _) => Some(Response(None)),
					_ => None,
				})
			}
		}

		eprintln!("Getting key {}/{} ...", key_vault_name, key_name);

		let (url, authorization) =
			self.key_vault_request_parameters(
				key_vault_name,
				&format!("/keys/{}?api-version=7.1", key_name),
			).await?;

		let response =
			self.client.request(
				hyper::Method::GET,
				&url,
				authorization,
				None::<&()>,
			).await?;
		Ok(match response {
			Response(Some(CreateOrGetKeyResponse { key })) => {
				eprintln!("Got key {}/{}: {:?}", key_vault_name, key_name, key);
				Some(key)
			},
			Response(None) => {
				eprintln!("Key {}/{} does not exist", key_vault_name, key_name);
				None
			},
		})
	}

	pub async fn key_vault_key_sign(
		&mut self,
		kid: &str,
		alg: &str,
		signature_input: &[u8],
	) -> anyhow::Result<String> {
		#[derive(serde::Serialize)]
		struct Request<'a> {
			alg: &'a str,
			value: &'a str,
		}

		#[derive(serde::Deserialize)]
		struct Response {
			value: String,
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

		eprintln!("Signing using key {} ...", kid);

		let url = format!("{}/sign?api-version=7.1", kid);
		let authorization = self.key_vault_authorization().await?;

		let value = http_common::jws_base64_encode(signature_input);

		let Response { value } =
			self.client.request(
				hyper::Method::POST,
				&url,
				authorization,
				Some(&Request {
					alg,
					value: &value,
				}),
			).await?;
		eprintln!("Got signature using key {}", kid);
		Ok(value)
	}
}

#[derive(Debug, serde::Deserialize)]
pub struct Key {
	pub crv: String,
	pub kid: String,
	pub kty: String,
	pub x: String,
	pub y: String,
}

#[derive(serde::Deserialize)]
struct CreateOrGetKeyResponse {
	key: Key,
}

impl http_common::FromResponse for CreateOrGetKeyResponse {
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
