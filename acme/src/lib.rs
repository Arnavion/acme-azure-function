#![deny(rust_2018_idioms, warnings)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(
	clippy::default_trait_access,
	clippy::let_underscore_drop,
	clippy::let_unit_value,
	clippy::missing_errors_doc,
	clippy::must_use_candidate,
	clippy::too_many_lines,
)]

use anyhow::Context;

pub struct Account<'a, K> {
	account_key: &'a K,
	account_url: Option<String>,
	client: http_common::Client,
	nonce: Option<http::HeaderValue>,
	new_nonce_url: http::Uri,
	new_order_url: http::Uri,
	logger: &'a log2::Logger,
}

impl<'a, K> Account<'a, K> where K: AccountKey {
	pub async fn new(
		acme_directory_url: http::Uri,
		acme_contact_url: &str,
		account_key: &'a K,
		user_agent: http::HeaderValue,
		logger: &'a log2::Logger,
	) -> anyhow::Result<Account<'a, K>> {
		#[derive(Debug, serde::Deserialize)]
		struct DirectoryResponse {
			#[serde(rename = "newAccount")]
			new_account_url: http_common::DeserializableUri,

			#[serde(rename = "newNonce")]
			new_nonce_url: http_common::DeserializableUri,

			#[serde(rename = "newOrder")]
			new_order_url: http_common::DeserializableUri,
		}

		impl http_common::FromResponse for DirectoryResponse {
			fn from_response(
				status: http::StatusCode,
				body: Option<(&http::HeaderValue, &mut http_common::Body<impl std::io::Read>)>,
				_headers: http::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				Ok(match (status, body) {
					(http::StatusCode::OK, Some((content_type, body))) if http_common::is_json(content_type) => Some(body.as_json()?),
					_ => None,
				})
			}
		}

		let client = http_common::Client::new(user_agent).context("could not create HTTP client")?;

		let (DirectoryResponse {
			new_account_url: http_common::DeserializableUri(new_account_url),
			new_nonce_url: http_common::DeserializableUri(new_nonce_url),
			new_order_url: http_common::DeserializableUri(new_order_url),
		}, log2::Secret(nonce)) = logger.report_operation("acme/directory", &acme_directory_url.clone(), <log2::ScopedObjectOperation>::Get, async {
			let mut req = http::Request::new(Default::default());
			*req.method_mut() = http::Method::GET;
			*req.uri_mut() = acme_directory_url;

			let ResponseWithNewNonce { body, new_nonce } = client.request(req).await.context("could not execute HTTP request")?;
			Ok((body, log2::Secret(new_nonce)))
		}).await.context("could not query ACME directory")?;

		let mut account = Account {
			account_key,
			account_url: None,
			client,
			nonce,
			new_nonce_url,
			new_order_url,
			logger,
		};

		account.account_url = {
			#[derive(serde::Serialize)]
			struct NewAccountRequest<'a> {
				#[serde(rename = "contact")]
				contact_urls: &'a [&'a str],

				#[serde(rename = "termsOfServiceAgreed")]
				terms_of_service_agreed: bool,
			}

			#[derive(serde::Deserialize)]
			struct NewAccountResponse {
				status: AccountStatus,
			}

			#[derive(Debug, serde::Deserialize)]
			enum AccountStatus {
				#[serde(rename = "deactivated")]
				Deactivated,
				#[serde(rename = "revoked")]
				Revoked,
				#[serde(rename = "valid")]
				Valid,
			}

			impl http_common::FromResponse for NewAccountResponse {
				fn from_response(
					status: http::StatusCode,
					body: Option<(&http::HeaderValue, &mut http_common::Body<impl std::io::Read>)>,
					_headers: http::HeaderMap,
				) -> anyhow::Result<Option<Self>> {
					Ok(match (status, body) {
						(
							http::StatusCode::CREATED | http::StatusCode::OK,
							Some((content_type, body)),
						) if http_common::is_json(content_type) => Some(body.as_json()?),
						_ => None,
					})
				}
			}

			let (account_url, status) = logger.report_operation("acme/account", "", <log2::ScopedObjectOperation>::Get, async {
				let http_common::ResponseWithLocation {
					body: NewAccountResponse { status },
					location: account_url,
				} =
					account.post(new_account_url, Some(&NewAccountRequest {
						contact_urls: &[acme_contact_url],
						terms_of_service_agreed: true,
					})).await.context("could not create / get account")?;
				Ok((account_url.to_string(), status))
			}).await?;

			logger.report_state("acme/account", &account_url, format_args!("{status:?}"));

			if !matches!(status, AccountStatus::Valid) {
				return Err(anyhow::anyhow!("Account has {status:?} status"));
			}

			Some(account_url)
		};

		Ok(account)
	}

	pub async fn place_order(&mut self, domain_name: &str) -> anyhow::Result<Order> {
		#[derive(serde::Serialize)]
		struct NewOrderRequest<'a> {
			identifiers: &'a [NewOrderRequestIdentifier<'a>],
		}

		#[derive(serde::Serialize)]
		struct NewOrderRequestIdentifier<'a> {
			r#type: &'a str,
			value: &'a str,
		}

		#[derive(Debug, serde::Deserialize)]
		struct OrderObjPending {
			#[serde(rename = "authorizations")]
			authorization_urls: Vec<http_common::DeserializableUri>,
		}

		let (order_url, mut order) = self.logger.report_operation("acme/order", domain_name, <log2::ScopedObjectOperation>::Get, async {
			let http_common::ResponseWithLocation {
				location: order_url,
				body: order,
			} =
				self.post(self.new_order_url.clone(), Some(&NewOrderRequest {
					identifiers: &[NewOrderRequestIdentifier {
						r#type: "dns",
						value: domain_name,
					}],
				})).await.context("could not create / get order")?;
			Ok((order_url, order))
		}).await?;

		let order = loop {
			self.logger.report_state("acme/order", &order_url, format_args!("{order:?}"));

			match order {
				OrderResponse::Pending(OrderObjPending { mut authorization_urls }) => {
					#[derive(Debug)]
					enum AuthorizationResponse {
						Pending { hasher: sha2::Sha256, challenge_url: http::Uri },
						Valid,
					}

					impl http_common::FromResponse for AuthorizationResponse {
						fn from_response(
							status: http::StatusCode,
							body: Option<(&http::HeaderValue, &mut http_common::Body<impl std::io::Read>)>,
							_headers: http::HeaderMap,
						) -> anyhow::Result<Option<Self>> {
							#[derive(serde::Deserialize)]
							struct AuthorizationPending<'a> {
								#[serde(borrow)]
								challenges: Vec<Challenge<ChallengePending<'a>>>,
							}

							#[derive(serde::Deserialize)]
							struct ChallengePending<'a> {
								#[serde(borrow)]
								token: std::borrow::Cow<'a, str>,
								#[serde(borrow)]
								r#type: std::borrow::Cow<'a, str>,
								url: http_common::DeserializableUri,
							}

							Ok(match (status, body) {
								(http::StatusCode::OK, Some((content_type, body))) if http_common::is_json(content_type) => Some(match body.as_json()? {
									Authorization::Pending(AuthorizationPending { challenges }) => {
										let (token, challenge_url) =
											challenges.into_iter()
											.find_map(|challenge| match challenge {
												Challenge::Pending(ChallengePending { token, r#type, url: http_common::DeserializableUri(url) }) =>
													(r#type == "dns-01").then_some((token, url)),
												Challenge::Processing |
												Challenge::Valid => None,
											})
											.context("did not find any pending dns-01 challenges")?;
										let mut hasher: sha2::Sha256 = sha2::Digest::new();
										sha2::Digest::update(&mut hasher, token.as_bytes());
										AuthorizationResponse::Pending { hasher, challenge_url }
									},

									Authorization::Valid => AuthorizationResponse::Valid,
								}),

								_ => None,
							})
						}
					}

					let http_common::DeserializableUri(authorization_url) = authorization_urls.pop().context("no authorizations")?;
					if !authorization_urls.is_empty() {
						return Err(anyhow::anyhow!("more than one authorization"));
					}

					let authorization = self.post(authorization_url.clone(), None::<&()>).await.context("could not get authorization")?;

					self.logger.report_state("acme/authorization", &authorization_url, format_args!("{authorization:?}"));

					let (mut hasher, challenge_url) =
						if let AuthorizationResponse::Pending { hasher, challenge_url } = authorization {
							(hasher, challenge_url)
						}
						else {
							return Err(anyhow::anyhow!("authorization has unexpected status"));
						};

					sha2::Digest::update(&mut hasher, b".");

					let jwk_thumbprint = {
						let mut hasher: sha2::Sha256 = sha2::Digest::new();
						let mut serializer = serde_json::Serializer::new(&mut hasher);
						let () = serde::Serialize::serialize(&self.account_key.as_jwk(), &mut serializer).expect("cannot fail to serialize JWK");
						sha2::Digest::finalize(hasher)
					};

					let hasher = {
						let mut writer = base64::write::EncoderWriter::new(hasher, JWS_BASE64_CONFIG);
						let () = std::io::Write::write_all(&mut writer, &jwk_thumbprint).expect("cannot fail to base64-encode JWK hash");
						writer.finish().expect("cannot fail to base64-encode JWK hash")
					};

					let hash = sha2::Digest::finalize(hasher);
					let dns_txt_record_content = base64::encode_config(&hash, JWS_BASE64_CONFIG);

					break Order::Pending(OrderPending {
						order_url,
						authorization_url,
						challenge_url,
						dns_txt_record_content,
					});
				},

				OrderResponse::Processing { retry_after } => {
					self.logger.report_message(format_args!("Waiting for {retry_after:?} before rechecking order..."));
					tokio::time::sleep(retry_after).await;
				},

				OrderResponse::Ready(serde::de::IgnoredAny) => break Order::Ready(OrderReady {
					order_url,
				}),

				OrderResponse::Valid { certificate_url } => break Order::Valid(OrderValid {
					certificate_url,
				}),
			};

			order = self.post(order_url.clone(), None::<&()>).await.context("could not get order")?;
		};

		Ok(order)
	}

	pub async fn complete_authorization(
		&mut self,
		OrderPending {
			order_url,
			authorization_url,
			challenge_url,
			dns_txt_record_content: _,
		}: OrderPending,
	) -> anyhow::Result<OrderReady> {
		#[derive(serde::Serialize)]
		struct ChallengeCompleteRequest { }

		#[derive(Debug)]
		enum ChallengeResponse {
			Pending,
			Processing { retry_after: std::time::Duration },
			Valid,
		}

		impl http_common::FromResponse for ChallengeResponse {
			fn from_response(
				status: http::StatusCode,
				body: Option<(&http::HeaderValue, &mut http_common::Body<impl std::io::Read>)>,
				headers: http::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				Ok(match (status, body) {
					(http::StatusCode::OK, Some((content_type, body))) if http_common::is_json(content_type) => Some(match body.as_json()? {
						Challenge::Pending(serde::de::IgnoredAny) => ChallengeResponse::Pending,

						Challenge::Processing => {
							let retry_after = http_common::get_retry_after(&headers, std::time::Duration::from_secs(1), std::time::Duration::from_secs(30))?;
							ChallengeResponse::Processing { retry_after }
						},

						Challenge::Valid => ChallengeResponse::Valid,
					}),
					_ => None,
				})
			}
		}

		#[derive(Debug)]
		enum AuthorizationResponse {
			Pending { retry_after: std::time::Duration },
			Valid,
		}

		impl http_common::FromResponse for AuthorizationResponse {
			fn from_response(
				status: http::StatusCode,
				body: Option<(&http::HeaderValue, &mut http_common::Body<impl std::io::Read>)>,
				headers: http::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				Ok(match (status, body) {
					(http::StatusCode::OK, Some((content_type, body))) if http_common::is_json(content_type) => Some(match body.as_json()? {
						Authorization::Pending(serde::de::IgnoredAny) => {
							let retry_after = http_common::get_retry_after(&headers, std::time::Duration::from_secs(1), std::time::Duration::from_secs(30))?;
							AuthorizationResponse::Pending { retry_after }
						},

						Authorization::Valid => AuthorizationResponse::Valid,
					}),

					_ => None,
				})
			}
		}

		self.logger.report_message(format_args!("Completing challenge {challenge_url} ..."));

		let mut body = Some(&ChallengeCompleteRequest { });
		loop {
			let challenge = self.post(challenge_url.clone(), body.take()).await.context("could not complete challenge")?;

			self.logger.report_state("acme/challenge", &challenge_url, format_args!("{challenge:?}"));

			match challenge {
				ChallengeResponse::Pending => {
					let retry_after = std::time::Duration::from_secs(1);
					self.logger.report_message(format_args!("Waiting for {retry_after:?} before rechecking challenge..."));
					tokio::time::sleep(retry_after).await;
				},

				ChallengeResponse::Processing { retry_after } => {
					self.logger.report_message(format_args!("Waiting for {retry_after:?} before rechecking challenge..."));
					tokio::time::sleep(retry_after).await;
				},

				ChallengeResponse::Valid => break,
			};
		}

		self.logger.report_message(format_args!("Waiting for authorization {authorization_url} ..."));

		loop {
			let authorization = self.post(authorization_url.clone(), None::<&()>).await.context("could not get authorization")?;

			self.logger.report_state("acme/authorization", &authorization_url, format_args!("{authorization:?}"));

			match authorization {
				AuthorizationResponse::Pending { retry_after } => {
					self.logger.report_message(format_args!("Waiting for {retry_after:?} before rechecking authorization..."));
					tokio::time::sleep(retry_after).await;
				},

				AuthorizationResponse::Valid => break,
			};
		}

		Ok(OrderReady {
			order_url,
		})
	}

	pub async fn finalize_order(
		&mut self,
		OrderReady {
			order_url,
		}: OrderReady,
		mut csr: String,
	) -> anyhow::Result<OrderValid> {
		#[derive(Debug, serde::Deserialize)]
		struct OrderObjReady {
			#[serde(rename = "finalize")]
			finalize_url: http_common::DeserializableUri,
		}

		self.logger.report_message(format_args!("Finalizing order {order_url} ..."));

		let order = loop {
			let order = self.post(order_url.clone(), None::<&()>).await.context("could not get order")?;

			self.logger.report_state("acme/order", &order_url, format_args!("{order:?}"));

			match order {
				OrderResponse::Pending(serde::de::IgnoredAny) => return Err(anyhow::anyhow!("order is still pending")),

				OrderResponse::Processing { retry_after } => {
					self.logger.report_message(format_args!("Waiting for {retry_after:?} before rechecking order..."));
					tokio::time::sleep(retry_after).await;
				},

				OrderResponse::Ready(OrderObjReady { finalize_url: http_common::DeserializableUri(finalize_url) }) => {
					#[derive(serde::Serialize)]
					struct FinalizeOrderRequest<'a> {
						csr: &'a str,
					}

					// SAFETY: libstd has no way to in-place replace some ASCII chars in a String with other ASCII chars.
					// `str::replace` always copies into a new String so it's wasteful for multiple replacements,
					// and `String::replace_range` requires a more complicated loop to find the next replacement site
					// and perform the replacement in a single pass.
					unsafe {
						for b in csr.as_bytes_mut() {
							match *b {
								b'+' => *b = b'-',
								b'/' => *b = b'_',
								_ => (),
							}
						}
					}
					let csr = csr.trim_end_matches('=');

					let _: OrderResponse<serde::de::IgnoredAny, serde::de::IgnoredAny> =
						self.post(finalize_url, Some(&FinalizeOrderRequest { csr })).await.context("could not finalize order")?;
				},

				OrderResponse::Valid { certificate_url } => break OrderValid {
					certificate_url,
				},
			};
		};

		Ok(order)
	}

	pub async fn download_certificate(
		&mut self,
		OrderValid {
			certificate_url,
		}: OrderValid,
	) -> anyhow::Result<String> {
		struct CertificateResponse(String);

		impl http_common::FromResponse for CertificateResponse {
			fn from_response(
				status: http::StatusCode,
				body: Option<(&http::HeaderValue, &mut http_common::Body<impl std::io::Read>)>,
				_headers: http::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				Ok(match (status, body) {
					(http::StatusCode::OK, Some((content_type, body))) if content_type == "application/pem-certificate-chain" => {
						let certificate = body.as_str()?.into_owned();
						Some(CertificateResponse(certificate))
					},
					_ => None,
				})
			}
		}

		let certificate = self.logger.report_operation("acme/certificate", &certificate_url.clone(), <log2::ScopedObjectOperation>::Get, async {
			let CertificateResponse(certificate) = self.post(certificate_url, None::<&()>).await.context("could not download certificate")?;
			Ok(certificate)
		}).await?;

		Ok(certificate)
	}

	async fn post<TRequest, TResponse>(
		&mut self,
		url: http::Uri,
		body: Option<&TRequest>,
	) -> anyhow::Result<TResponse>
	where
		TRequest: serde::Serialize,
		TResponse: http_common::FromResponse,
	{
		// This fn encapsulates the non-generic parts of `post` to reduce code size from monomorphization.
		async fn make_request<K>(account: &mut Account<'_, K>, url: http::Uri, payload: String) -> anyhow::Result<http::Request<hyper::Body>> where K: AccountKey {
			#[allow(clippy::declare_interior_mutable_const)] // Clippy doesn't like const http::HeaderValue
			const APPLICATION_JOSE_JSON: http::HeaderValue = http::HeaderValue::from_static("application/jose+json");

			let nonce =
				if let Some(nonce) = account.nonce.take() {
					nonce
				}
				else {
					struct NewNonceResponse;

					impl http_common::FromResponse for NewNonceResponse {
						fn from_response(
							status: http::StatusCode,
							_body: Option<(&http::HeaderValue, &mut http_common::Body<impl std::io::Read>)>,
							_headers: http::HeaderMap,
						) -> anyhow::Result<Option<Self>> {
							Ok(match status {
								http::StatusCode::OK => Some(NewNonceResponse),
								_ => None,
							})
						}
					}

					let log2::Secret(nonce) = account.logger.report_operation("acme/nonce", "", <log2::ScopedObjectOperation>::Get, async {
						let mut req = http::Request::new(Default::default());
						*req.method_mut() = http::Method::HEAD;
						*req.uri_mut() = account.new_nonce_url.clone();

						let ResponseWithNewNonce::<NewNonceResponse> { body: _, new_nonce } =
							account.client.request(req).await
							.context("could not execute HTTP request")
							.context("newNonce URL did not return new nonce")?;

						let nonce = new_nonce.context("newNonce URL did not return new nonce")?;
						Ok(log2::Secret(nonce))
					}).await?;
					nonce
				};

			let protected = {
				#[derive(serde::Serialize)]
				struct Protected<'a> {
					alg: &'a str,

					#[serde(flatten)]
					jwk_or_kid: JwkOrKid<'a>,

					#[serde(serialize_with = "serialize_header_value")]
					nonce: &'a http::HeaderValue,

					url: std::fmt::Arguments<'a>,
				}

				#[derive(serde::Serialize)]
				enum JwkOrKid<'a> {
					#[serde(rename = "jwk")]
					Jwk(Jwk<'a>),

					#[serde(rename = "kid")]
					Kid(&'a str),
				}

				fn serialize_header_value<S>(header_value: &http::HeaderValue, serializer: S) -> Result<S::Ok, S::Error> where S: serde::Serializer {
					let header_value = header_value.to_str().map_err(serde::ser::Error::custom)?;
					serializer.serialize_str(header_value)
				}

				let jwk = account.account_key.as_jwk();
				let alg = jwk.crv.jws_sign_alg();

				let jwk_or_kid = account.account_url.as_deref().map_or_else(|| JwkOrKid::Jwk(jwk), JwkOrKid::Kid);

				let mut writer = base64::write::EncoderStringWriter::from(String::with_capacity(1024), JWS_BASE64_CONFIG);
				let mut serializer = serde_json::Serializer::new(&mut writer);
				let () =
					serde::Serialize::serialize(
						&Protected {
							alg,
							jwk_or_kid,
							nonce: &nonce,
							url: format_args!("{url}"),
						},
						&mut serializer,
					).context("could not serialize `protected`")?;
				writer.into_inner()
			};

			let signature =
				account.account_key.sign([
					protected.as_bytes(),
					&b"."[..],
					payload.as_bytes(),
				]).await?;

			// All strings are base64 so there's no need to get serde_json involved
			let body = {
				#[allow(clippy::declare_interior_mutable_const)] // Clippy doesn't like const hyper::body::Bytes
				const PART1: hyper::body::Bytes = hyper::body::Bytes::from_static(br#"{"payload":""#);
				#[allow(clippy::declare_interior_mutable_const)] // Clippy doesn't like const hyper::body::Bytes
				const PART2: hyper::body::Bytes = hyper::body::Bytes::from_static(br#"","protected":""#);
				#[allow(clippy::declare_interior_mutable_const)] // Clippy doesn't like const hyper::body::Bytes
				const PART3: hyper::body::Bytes = hyper::body::Bytes::from_static(br#"","signature":""#);
				#[allow(clippy::declare_interior_mutable_const)] // Clippy doesn't like const hyper::body::Bytes
				const PART4: hyper::body::Bytes = hyper::body::Bytes::from_static(br#""}"#);

				let body =
					futures_util::stream::iter([
						Ok::<_, std::convert::Infallible>(PART1),
						Ok(payload.into()),
						Ok(PART2),
						Ok(protected.into()),
						Ok(PART3),
						Ok(signature.into()),
						Ok(PART4),
					]);

				hyper::Body::wrap_stream(body)
			};

			let mut req = http::Request::new(body);
			*req.method_mut() = http::Method::POST;
			*req.uri_mut() = url;
			req.headers_mut().insert(http::header::CONTENT_TYPE, APPLICATION_JOSE_JSON);
			Ok(req)
		}

		let payload =
			if let Some(payload) = body {
				let mut writer = base64::write::EncoderStringWriter::new(JWS_BASE64_CONFIG);
				let mut serializer = serde_json::Serializer::new(&mut writer);
				let () = serde::Serialize::serialize(payload, &mut serializer).context("could not serialize `payload`")?;
				writer.into_inner()
			}
			else {
				String::new()
			};

		let req = make_request(self, url, payload).await?;

		let ResponseWithNewNonce { body, new_nonce } =
			self.client.request(req).await.context("could not execute HTTP request")?;

		self.nonce = new_nonce;

		Ok(body)
	}
}

pub trait AccountKey {
	fn as_jwk(&self) -> Jwk<'_>;

	fn sign<'a, I>(&'a self, digest: I) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<String>> + 'a>>
	where
		I: IntoIterator,
		<I as IntoIterator>::Item: AsRef<[u8]>;
}

#[derive(Clone, Copy, serde::Serialize)]
pub struct Jwk<'a> {
	pub crv: EcCurve,
	pub kty: &'a str,
	pub x: &'a str,
	pub y: &'a str,
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

impl EcCurve {
	pub const fn jws_sign_alg(self) -> &'static str {
		match self {
			EcCurve::P256 => "ES256",
			EcCurve::P384 => "ES384",
			EcCurve::P521 => "ES512",
		}
	}
}

pub enum Order {
	Pending(OrderPending),
	Ready(OrderReady),
	Valid(OrderValid),
}

pub struct OrderPending {
	order_url: http::Uri,
	authorization_url: http::Uri,
	challenge_url: http::Uri,
	pub dns_txt_record_content: String,
}

pub struct OrderReady {
	order_url: http::Uri,
}

pub struct OrderValid {
	certificate_url: http::Uri,
}

pub const JWS_BASE64_CONFIG: base64::Config = base64::Config::new(base64::CharacterSet::UrlSafe, false);

struct ResponseWithNewNonce<TResponse> {
	body: TResponse,
	new_nonce: Option<http::HeaderValue>,
}

impl<TResponse> http_common::FromResponse for ResponseWithNewNonce<TResponse> where TResponse: http_common::FromResponse {
	fn from_response(
		status: http::StatusCode,
		body: Option<(&http::HeaderValue, &mut http_common::Body<impl std::io::Read>)>,
		mut headers: http::HeaderMap,
	) -> anyhow::Result<Option<Self>> {
		#[allow(clippy::declare_interior_mutable_const)] // Clippy doesn't like const http::HeaderName
		const REPLAY_NONCE: http::header::HeaderName = http::header::HeaderName::from_static("replay-nonce");

		#[allow(clippy::borrow_interior_mutable_const)] // Clippy doesn't like const http::HeaderName
		let new_nonce = headers.remove(&REPLAY_NONCE);
		match TResponse::from_response(status, body, headers) {
			Ok(Some(body)) => Ok(Some(ResponseWithNewNonce { body, new_nonce })),
			Ok(None) => Ok(None),
			Err(err) => Err(err),
		}
	}
}

#[derive(Debug)]
enum OrderResponse<TPending, TReady> {
	Pending(TPending),

	Processing {
		retry_after: std::time::Duration,
	},

	Ready(TReady),

	Valid {
		certificate_url: http::Uri,
	},
}

impl<TPending, TReady> http_common::FromResponse for OrderResponse<TPending, TReady>
where
	TPending: serde::de::DeserializeOwned,
	TReady: serde::de::DeserializeOwned,
{
	fn from_response(
		status: http::StatusCode,
		body: Option<(&http::HeaderValue, &mut http_common::Body<impl std::io::Read>)>,
		headers: http::HeaderMap,
	) -> anyhow::Result<Option<Self>> {
		#[derive(serde::Deserialize)]
		#[serde(tag = "status")]
		enum Order<TPending, TReady> {
			#[serde(rename = "pending")]
			Pending(TPending),

			#[serde(rename = "processing")]
			Processing,

			#[serde(rename = "ready")]
			Ready(TReady),

			#[serde(rename = "valid")]
			Valid {
				#[serde(rename = "certificate")]
				certificate_url: http_common::DeserializableUri,
			},
		}

		Ok(match (status, body) {
			(
				http::StatusCode::CREATED | http::StatusCode::OK,
				Some((content_type, body)),
			) if http_common::is_json(content_type) => Some(match body.as_json()? {
				Order::Pending(pending) => OrderResponse::Pending(pending),

				Order::Processing => {
					let retry_after = http_common::get_retry_after(&headers, std::time::Duration::from_secs(1), std::time::Duration::from_secs(30))?;
					OrderResponse::Processing { retry_after }
				},

				Order::Ready(ready) => OrderResponse::Ready(ready),

				Order::Valid { certificate_url: http_common::DeserializableUri(certificate_url) } => OrderResponse::Valid { certificate_url },
			}),

			_ => None,
		})
	}
}

#[derive(serde::Deserialize)]
#[serde(tag = "status")]
enum Authorization<TPending> {
	#[serde(rename = "pending")]
	Pending(TPending),

	#[serde(rename = "valid")]
	Valid,
}

#[derive(serde::Deserialize)]
#[serde(tag = "status")]
enum Challenge<TPending> {
	#[serde(rename = "pending")]
	Pending(TPending),

	#[serde(rename = "processing")]
	Processing,

	#[serde(rename = "valid")]
	Valid,
}
