impl<'a> super::Client<'a> {
	pub async fn key_create<'b>(
		&'b self,
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
				(self.key_vault_name, key_name),
				log2::ScopedObjectOperation::Create { value: format_args!("{:?}", (kty, crv)) },
				async {
					let (url, authorization) = self.request_parameters(format_args!("/keys/{}/create?api-version=7.1", key_name)).await?;

					let CreateOrGetKeyResponse { key } =
						self.client.request(
							hyper::Method::POST,
							url,
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
			client: self,
		})
	}

	pub async fn key_get<'b>(
		&'b self,
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

		let key = log2::report_operation("azure/key_vault/key", (self.key_vault_name, key_name), <log2::ScopedObjectOperation>::Get, async {
			let (url, authorization) = self.request_parameters(format_args!("/keys/{}?api-version=7.1", key_name)).await?;

			let Response(response) =
				self.client.request(
					hyper::Method::GET,
					url,
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
			client: self,
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
	client: &'a super::Client<'a>,
}

impl acme::AccountKey for Key<'_> {
	fn jwk(&self) -> acme::Jwk<'_> {
		acme::Jwk {
			crv: match self.crv {
				super::EcCurve::P256 => acme::EcCurve::P256,
				super::EcCurve::P384 => acme::EcCurve::P384,
				super::EcCurve::P521 => acme::EcCurve::P521,
			},
			kty: "EC",
			x: &self.x,
			y: &self.y,
		}
	}

	fn sign<'a>(
		&'a self,
		alg: &'static str,
		digest: &'a str,
	) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<String>> + 'a>> {
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

		Box::pin(log2::report_operation("azure/key_vault/key/signature", &self.kid, log2::ScopedObjectOperation::Create { value: "" }, async move {
			let url = format!("{}/sign?api-version=7.1", self.kid);
			let authorization = self.client.authorization().await?;

			let KeyVaultSignResponse { value: signature } =
				self.client.client.request(
					hyper::Method::POST,
					url,
					authorization,
					Some(&KeyVaultSignRequest {
						alg,
						value: digest,
					}),
				).await?;
			Ok::<_, anyhow::Error>(signature)
		}))
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
