use anyhow::Context;

impl<'a> crate::Account<'a> {
	pub async fn key_vault_key_create<'b>(
		&'b self,
		key_vault_name: &str,
		key_name: &str,
		kty: EcKty,
		crv: super::EcCurve,
	) -> anyhow::Result<Key<'b>> {
		#[derive(serde::Serialize)]
		struct Request<'a> {
			crv: super::EcCurve,
			kty: EcKty,
			key_ops: &'a [&'a str],
		}

		let key =
			log2::report_operation(
				"azure/key_vault/key",
				&format!("{}/{}", key_vault_name, key_name),
				log2::ScopedObjectOperation::Create { value: &format!("{:?}", (kty, crv)) },
				async {
					let key_vault_request_parameters =
						self.key_vault_request_parameters(
							key_vault_name,
							format_args!("/keys/{}/create?api-version=7.1", key_name),
						);
					let (url, authorization) = key_vault_request_parameters.await?;

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
					Ok::<_, anyhow::Error>(key)
				},
			).await?;

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

		let key = log2::report_operation("azure/key_vault/key", &format!("{}/{}", key_vault_name, key_name), log2::ScopedObjectOperation::Get, async {
			let key_vault_request_parameters =
				self.key_vault_request_parameters(
					key_vault_name,
					format_args!("/keys/{}?api-version=7.1", key_name),
				);
			let (url, authorization) = key_vault_request_parameters.await?;

			let Response(response) =
				self.client.request(
					hyper::Method::GET,
					&url,
					authorization,
					None::<&()>,
				).await?;
			let key = response.map(|CreateOrGetKeyResponse { key }| key);
			Ok::<_, anyhow::Error>(key)
		}).await?;

		Ok(key.map(|key| Key {
			crv: key.crv,
			kid: key.kid,
			x: key.x,
			y: key.y,
			account: self,
		}))
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
	crv: super::EcCurve,
	kid: String,
	x: String,
	y: String,
	account: &'a crate::Account<'a>,
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

	pub fn jws_alg(&self) -> &'static str {
		match self.crv {
			super::EcCurve::P256 => "ES256",
			super::EcCurve::P384 => "ES384",
			super::EcCurve::P521 => "ES512",
		}
	}

	pub async fn jws<TProtected, TPayload>(
		&self,
		protected: TProtected,
		payload: Option<TPayload>,
	) -> anyhow::Result<Vec<u8>>
	where
		TProtected: serde::Serialize,
		TPayload: serde::Serialize,
	{
		macro_rules! hash {
			($crv:expr, $protected:expr, $payload:expr, { $($crv_name:pat => $hash:ty,)* }) => {
				match $crv {
					$(
						$crv_name => {
							let mut hasher: $hash = sha2::Digest::new();
							sha2::Digest::update(&mut hasher, $protected);
							sha2::Digest::update(&mut hasher, b".");
							sha2::Digest::update(&mut hasher, $payload);
							let hash = sha2::Digest::finalize(hasher);
							http_common::jws_base64_encode(&hash)
						},
					)*
				}
			};
		}

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

		let protected = serde_json::to_vec(&protected).context("could not serialize `protected`")?;
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
			let alg = self.jws_alg();

			let hash = hash!(self.crv, &protected, &payload, {
				super::EcCurve::P256 => sha2::Sha256,
				super::EcCurve::P384 => sha2::Sha384,
				super::EcCurve::P521 => sha2::Sha512,
			});

			let signature = log2::report_operation("azure/key_vault/key/signature", &self.kid, log2::ScopedObjectOperation::Create { value: "" }, async {
				let url = format!("{}/sign?api-version=7.1", self.kid);
				let authorization = self.account.key_vault_authorization().await?;

				let KeyVaultSignResponse { value: signature } =
					self.account.client.request(
						hyper::Method::POST,
						&url,
						authorization,
						Some(&KeyVaultSignRequest {
							alg,
							value: &hash,
						}),
					).await?;
				Ok::<_, anyhow::Error>(signature)
			}).await?;

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
	crv: super::EcCurve,
	kty: &'a str,
	x: &'a str,
	y: &'a str,
}

impl Jwk<'_> {
	pub fn thumbprint(&self) -> String {
		let jwk = serde_json::to_vec(self).expect("could not compute JWK thumbprint");
		let mut hasher: sha2::Sha256 = sha2::Digest::new();
		sha2::Digest::update(&mut hasher, &jwk);
		let hash = sha2::Digest::finalize(hasher);
		http_common::jws_base64_encode(&hash)
	}
}

#[derive(serde::Deserialize)]
struct CreateOrGetKeyResponse {
	key: KeyResponse,
}

#[derive(Debug, serde::Deserialize)]
struct KeyResponse {
	crv: super::EcCurve,
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
