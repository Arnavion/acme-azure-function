use anyhow::Context;

impl super::Client<'_> {
	pub async fn key_create<'b>(
		&'b self,
		key_name: &str,
		kty: EcKty,
		crv: acme::EcCurve,
	) -> anyhow::Result<Key<'b>> {
		#[derive(serde::Serialize)]
		struct Request<'a> {
			crv: acme::EcCurve,
			kty: EcKty,
			key_ops: &'a [&'a str],
		}

		let key =
			self.logger.report_operation(
				"azure/key_vault/key",
				(self.key_vault_name, key_name),
				log2::ScopedObjectOperation::Create { value: format_args!("{:?}", (kty, crv)) },
				async {
					let CreateOrGetKeyResponse { key } =
						crate::request(
							self,
							http_common::Method::POST,
							format_args!("/keys/{key_name}/create?api-version=7.4"),
							Some(&Request {
								crv,
								kty,
								key_ops: &["sign", "verify"],
							}),
						).await?;
					Ok::<_, anyhow::Error>(key)
				},
			).await?;

		let key = Key::new(key, self)?;
		Ok(key)
	}

	pub async fn key_get<'b>(
		&'b self,
		key_name: &str,
	) -> anyhow::Result<Option<Key<'b>>> {
		struct Response(Option<CreateOrGetKeyResponse>);

		impl http_common::FromResponse for Response {
			fn from_response(
				status: http_common::StatusCode,
				body: Option<&mut http_common::ResponseBody<impl std::io::Read>>,
				_headers: http_common::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				Ok(match (status, body) {
					(http_common::StatusCode::OK, Some(body)) => Some(Response(Some(body.as_json()?))),
					(http_common::StatusCode::NOT_FOUND, _) => Some(Response(None)),
					_ => None,
				})
			}
		}

		let key = self.logger.report_operation("azure/key_vault/key", (self.key_vault_name, key_name), <log2::ScopedObjectOperation>::Get, async {
			let Response(response) =
				crate::request(
					self,
					http_common::Method::GET,
					format_args!("/keys/{key_name}?api-version=7.4"),
					None::<&()>,
				).await?;
			let key = response.map(|CreateOrGetKeyResponse { key }| key);
			Ok::<_, anyhow::Error>(key)
		}).await?;

		let key = key.map(|key| Key::new(key, self)).transpose()?;
		Ok(key)
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
	crv: acme::EcCurve,
	kid: String,
	x: String,
	y: String,
	client: &'a super::Client<'a>,
	sign_url: http_common::Uri,
}

impl acme::AccountKey for Key<'_> {
	fn as_jwk(&self) -> acme::Jwk<'_> {
		acme::Jwk {
			crv: self.crv,
			kty: "EC",
			x: &self.x,
			y: &self.y,
		}
	}

	fn sign<I>(&self, digest: I) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<String>> + '_>>
	where
		I: IntoIterator,
		<I as IntoIterator>::Item: AsRef<[u8]>,
	{
		macro_rules! hash {
			($crv:expr, $digest:expr, { $($crv_name:pat => $hasher:ty ,)* }) => {
				match $crv {
					$(
						$crv_name => {
							let hasher: $hasher = $digest.into_iter().fold(Default::default(), sha2::Digest::chain_update);
							let hash = sha2::Digest::finalize(hasher);
							let hash = base64::Engine::encode(&acme::JWS_BASE64_ENGINE, hash);
							hash
						},
					)*
				}
			};
		}

		// This fn encapsulates the non-generic parts of `sign` to reduce code size from monomorphization.
		async fn sign_inner(key: &Key<'_>, digest: String) -> anyhow::Result<String> {
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
					status: http_common::StatusCode,
					body: Option<&mut http_common::ResponseBody<impl std::io::Read>>,
					_headers: http_common::HeaderMap,
				) -> anyhow::Result<Option<Self>> {
					Ok(match (status, body) {
						(http_common::StatusCode::OK, Some(body)) => Some(body.as_json()?),
						_ => None,
					})
				}
			}

			let alg = key.crv.jws_sign_alg();

			let signature = key.client.logger.report_operation("azure/key_vault/key/signature", &key.kid, log2::ScopedObjectOperation::Create { value: "" }, async move {
				let KeyVaultSignResponse { value: signature } =
					crate::request(
						key.client,
						http_common::Method::POST,
						key.sign_url.clone(),
						Some(&KeyVaultSignRequest {
							alg,
							value: &digest,
						}),
					).await?;
				Ok::<_, anyhow::Error>(signature)
			}).await?;
			Ok(signature)
		}

		let digest = hash!(self.crv, digest, {
			acme::EcCurve::P256 => sha2::Sha256,
			acme::EcCurve::P384 => sha2::Sha384,
			acme::EcCurve::P521 => sha2::Sha512,
		});

		Box::pin(sign_inner(self, digest))
	}
}

#[derive(serde::Deserialize)]
struct CreateOrGetKeyResponse {
	key: KeyResponse,
}

#[derive(Debug, serde::Deserialize)]
struct KeyResponse {
	crv: acme::EcCurve,
	kid: String,
	// rustc thinks this field is unused, but it's being used to assert the key is one of the EC types,
	// or else the deserialization wouldn't succeed.
	#[allow(unused)]
	kty: EcKty,
	x: String,
	y: String,
}

impl http_common::FromResponse for CreateOrGetKeyResponse {
	fn from_response(
		status: http_common::StatusCode,
		body: Option<&mut http_common::ResponseBody<impl std::io::Read>>,
		_headers: http_common::HeaderMap,
	) -> anyhow::Result<Option<Self>> {
		Ok(match (status, body) {
			(http_common::StatusCode::OK, Some(body)) => Some(body.as_json()?),
			_ => None,
		})
	}
}

impl<'a> Key<'a> {
	fn new(key: KeyResponse, client: &'a super::Client<'a>) -> anyhow::Result<Self> {
		let sign_url = format!("{}/sign?api-version=7.4", key.kid).try_into().context("could not construct sign URL")?;

		Ok(Key {
			crv: key.crv,
			kid: key.kid,
			x: key.x,
			y: key.y,
			client,
			sign_url,
		})
	}
}
