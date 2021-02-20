use anyhow::Context;

impl<'a> crate::Account<'a> {
	pub async fn key_vault_key_create<'b>(
		&'b self,
		key_vault_name: &str,
		key_name: &str,
		kty: EcKty,
		crv: EcCurve,
	) -> anyhow::Result<Key<'b>> {
		#[derive(serde::Serialize)]
		struct Request<'a> {
			crv: EcCurve,
			kty: EcKty,
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
					crv,
					kty,
					key_ops: &["sign", "verify"],
				}),
			).await?;

		eprintln!("Created key {}/{}: {:?}", key_vault_name, key_name, key);

		Ok(Key {
			crv: key.crv,
			kid: key.kid,
			x: key.x,
			y: key.y,
			account: self,
		})
	}

	pub async fn key_vault_key_get<'b>(
		&'b self,
		key_vault_name: &str,
		key_name: &str,
	) -> anyhow::Result<Option<Key<'b>>> {
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
				Some(Key {
					crv: key.crv,
					kid: key.kid,
					x: key.x,
					y: key.y,
					account: self,
				})
			},
			Response(None) => {
				eprintln!("Key {}/{} does not exist", key_vault_name, key_name);
				None
			},
		})
	}
}

#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize)]
pub enum EcKty {
	#[serde(rename = "EC")]
	Ec,

	#[serde(rename = "EC-HSM")]
	EcHsm,
}

pub struct Key<'a> {
	pub crv: EcCurve,
	pub kid: String,
	pub x: String,
	pub y: String,
	account: &'a crate::Account<'a>,
}

#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize)]
pub enum EcCurve {
	#[serde(rename = "P-256")]
	P256,

	#[serde(rename = "P-384")]
	P384,

	#[serde(rename = "P-521")]
	P521,
}

impl Key<'_> {
	pub fn jwk(&self) -> Jwk<'_> {
		Jwk {
			crv: self.crv,
			kty: "EC",
			x: &self.x,
			y: &self.y,
		}
	}

	pub async fn jws<FProtected, TPayload>(
		&self,
		payload: Option<TPayload>,
		protected: FProtected,
	) -> anyhow::Result<Vec<u8>>
	where
		FProtected: FnOnce(&str) -> Vec<u8>,
		TPayload: serde::Serialize,
	{
		#[derive(serde::Serialize)]
		struct KeyVaultSignRequest<'a> {
			alg: &'a str,
			value: &'a str,
		}

		#[derive(serde::Deserialize)]
		struct KeyVaultSignResponse {
			value: String,
		}

		impl http_common::FromResponse for KeyVaultSignResponse {
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

		#[derive(serde::Serialize)]
		struct JwsRequest<'a> {
			payload: &'a str,
			protected: &'a str,
			signature: &'a str,
		}

		let (alg, hash): (&'static str, fn(&str, &str) -> String) = match self.crv {
			EcCurve::P256 => (
				"ES256",
				|protected, payload| {
					let mut hasher: sha2::Sha256 = sha2::Digest::new();
					sha2::Digest::update(&mut hasher, &protected);
					sha2::Digest::update(&mut hasher, b".");
					sha2::Digest::update(&mut hasher, &payload);
					let signature_input = sha2::Digest::finalize(hasher);
					http_common::jws_base64_encode(&signature_input)
				},
			),

			EcCurve::P384 => (
				"ES384",
				|protected, payload| {
					let mut hasher: sha2::Sha384 = sha2::Digest::new();
					sha2::Digest::update(&mut hasher, &protected);
					sha2::Digest::update(&mut hasher, b".");
					sha2::Digest::update(&mut hasher, &payload);
					let signature_input = sha2::Digest::finalize(hasher);
					http_common::jws_base64_encode(&signature_input)
				},
			),

			EcCurve::P521 => (
				"ES512",
				|protected, payload| {
					let mut hasher: sha2::Sha512 = sha2::Digest::new();
					sha2::Digest::update(&mut hasher, &protected);
					sha2::Digest::update(&mut hasher, b".");
					sha2::Digest::update(&mut hasher, &payload);
					let signature_input = sha2::Digest::finalize(hasher);
					http_common::jws_base64_encode(&signature_input)
				},
			),
		};

		let protected = protected(alg);
		let protected = http_common::jws_base64_encode(&protected);

		let payload =
			if let Some(payload) = payload {
				let payload = serde_json::to_vec(&payload).context("could not serialize `payload`")?;
				let payload = http_common::jws_base64_encode(&payload);
				payload
			}
			else {
				String::new()
			};

		let signature = {
			let signature_input = hash(&protected, &payload);

			eprintln!("Signing using key {} ...", self.kid);

			let url = format!("{}/sign?api-version=7.1", self.kid);
			let authorization = self.account.key_vault_authorization().await?;

			let KeyVaultSignResponse { value: signature } =
				self.account.client.request(
					hyper::Method::POST,
					&url,
					authorization,
					Some(&KeyVaultSignRequest {
						alg,
						value: &signature_input,
					}),
				).await?;
			eprintln!("Got signature using key {}", self.kid);
			signature
		};

		let body = JwsRequest {
			payload: &payload,
			protected: &protected,
			signature: &signature,
		};
		let body = serde_json::to_vec(&body).expect("could not serialize JWS request body");
		Ok(body)
	}
}

#[derive(serde::Serialize)]
pub struct Jwk<'a> {
	crv: EcCurve,
	kty: &'a str,
	x: &'a str,
	y: &'a str,
}

impl Jwk<'_> {
	pub fn thumbprint(&self) -> String {
		let jwk = serde_json::to_vec(self).expect("could not compute JWK thumbprint");
		let mut hasher: sha2::Sha256 = sha2::Digest::new();
		sha2::Digest::update(&mut hasher, &jwk);
		let result = sha2::Digest::finalize(hasher);
		http_common::jws_base64_encode(&result)
	}
}

#[derive(serde::Deserialize)]
struct CreateOrGetKeyResponse {
	key: KeyResponse,
}

#[derive(Debug, serde::Deserialize)]
struct KeyResponse {
	crv: EcCurve,
	kid: String,
	kty: EcKty,
	x: String,
	y: String,
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
