use anyhow::Context;

pub struct Client<'a> {
	inner: http_common::Client,

	new_account_url: http_common::Uri,
	new_nonce_url: http_common::Uri,
	new_order_url: http_common::Uri,
	renewal_info_url: Option<http_common::Uri>,

	logger: &'a log2::Logger,
}

pub struct Account<'a, K> {
	inner: http_common::Client,

	new_nonce_url: http_common::Uri,
	new_order_url: http_common::Uri,

	logger: &'a log2::Logger,

	account_key: &'a K,
	account_url: Option<String>,

	nonce: Option<http_common::HeaderValue>,
}

impl<'a> Client<'a> {
	pub async fn new(
		acme_directory_url: http_common::Uri,
		user_agent: http_common::HeaderValue,
		logger: &'a log2::Logger,
	) -> anyhow::Result<Self> {
		#[derive(Debug, serde::Deserialize)]
		struct DirectoryResponse {
			#[serde(rename = "newAccount")]
			new_account_url: http_common::DeserializableUri,

			#[serde(rename = "newNonce")]
			new_nonce_url: http_common::DeserializableUri,

			#[serde(rename = "newOrder")]
			new_order_url: http_common::DeserializableUri,

			#[serde(rename = "renewalInfo")]
			renewal_info_url: Option<http_common::DeserializableUri>,
		}

		impl http_common::FromResponse for DirectoryResponse {
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

		let inner = http_common::Client::new(user_agent).context("could not create HTTP client")?;

		let DirectoryResponse {
			new_account_url: http_common::DeserializableUri(new_account_url),
			new_nonce_url: http_common::DeserializableUri(new_nonce_url),
			new_order_url: http_common::DeserializableUri(new_order_url),
			renewal_info_url,
		} = logger.report_operation("acme/directory", &acme_directory_url.clone(), <log2::ScopedObjectOperation>::Get, async {
			let mut req = http_common::Request::new(Default::default());
			*req.method_mut() = http_common::Method::GET;
			*req.uri_mut() = acme_directory_url;

			let body = inner.request(req).await.context("could not execute HTTP request")?;
			Ok::<_, anyhow::Error>(body)
		}).await.context("could not query ACME directory")?;

		Ok(Client {
			inner,
			new_account_url,
			new_nonce_url,
			new_order_url,
			renewal_info_url: renewal_info_url.map(|http_common::DeserializableUri(renewal_info_url)| renewal_info_url),
			logger,
		})
	}

	pub async fn renewal_suggested_window_start(&mut self, ari_id: &str) -> anyhow::Result<Option<time::OffsetDateTime>> {
		#[derive(Debug, serde::Deserialize)]
		struct Response {
			#[serde(rename = "suggestedWindow")]
			suggested_window: SuggestedWindow,
		}

		#[derive(Debug, serde::Deserialize)]
		struct SuggestedWindow {
			#[serde(deserialize_with = "time::serde::rfc3339::deserialize")]
			start: time::OffsetDateTime,
		}

		impl http_common::FromResponse for Response {
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

		let Some(renewal_info_url) = self.renewal_info_url.as_ref() else { return Ok(None) };

		let mut renewal_info_url = renewal_info_url.to_string();
		if !renewal_info_url.ends_with('/') {
			renewal_info_url.push('/');
		}
		renewal_info_url.push_str(ari_id);
		let renewal_info_url: http_common::Uri = renewal_info_url.try_into().context("could not construct renewal info URL")?;

		let response = self.logger.report_operation("acme/renewalInfo", &renewal_info_url.clone(), <log2::ScopedObjectOperation>::Get, async {
			let mut req = http_common::Request::new(Default::default());
			*req.method_mut() = http_common::Method::GET;
			*req.uri_mut() = renewal_info_url;

			// Suppress ARI errors so that the caller falls back to not-after-based calculation.
			let body = self.inner.request(req).await.ok();
			Ok::<_, anyhow::Error>(body)
		}).await.context("could not query ACME renewal info")?;

		let start = response.map(|Response { suggested_window: SuggestedWindow { start } }| start);
		Ok(start)
	}

	pub async fn new_account<K>(
		self,
		acme_contact_url: &str,
		account_key: &'a K,
	) -> anyhow::Result<Account<'a, K>>
	where
		K: AccountKey,
	{
		let Client {
			inner,

			new_account_url,
			new_nonce_url,
			new_order_url,
			renewal_info_url: _,

			logger,
		} = self;

		let mut account = Account {
			inner,

			new_nonce_url,
			new_order_url,

			logger,

			account_key,
			account_url: None,

			nonce: None,
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
					status: http_common::StatusCode,
					body: Option<&mut http_common::ResponseBody<impl std::io::Read>>,
					_headers: http_common::HeaderMap,
				) -> anyhow::Result<Option<Self>> {
					Ok(match (status, body) {
						(http_common::StatusCode::CREATED | http_common::StatusCode::OK, Some(body)) => Some(body.as_json()?),
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
				Ok::<_, anyhow::Error>((account_url.to_string(), status))
			}).await?;

			logger.report_state("acme/account", &account_url, format_args!("{status:?}"));

			if !matches!(status, AccountStatus::Valid) {
				return Err(anyhow::anyhow!("Account has {status:?} status"));
			}

			Some(account_url)
		};

		Ok(account)
	}
}

impl<K> Account<'_, K> where K: AccountKey {
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
					identifiers: &[
						NewOrderRequestIdentifier {
							r#type: "dns",
							value: domain_name,
						},
						NewOrderRequestIdentifier {
							r#type: "dns",
							value: &format!("*.{domain_name}"),
						},
					],
				})).await.context("could not create / get order")?;
			Ok::<_, anyhow::Error>((order_url, order))
		}).await?;

		let order = loop {
			self.logger.report_state("acme/order", &order_url, format_args!("{order:?}"));

			match order {
				OrderResponse::Pending(OrderObjPending { authorization_urls }) => {
					#[derive(Debug)]
					enum AuthorizationResponse {
						Pending { hasher: sha2::Sha256, challenge_url: http_common::Uri },
						Valid,
					}

					impl http_common::FromResponse for AuthorizationResponse {
						fn from_response(
							status: http_common::StatusCode,
							body: Option<&mut http_common::ResponseBody<impl std::io::Read>>,
							_headers: http_common::HeaderMap,
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
								(http_common::StatusCode::OK, Some(body)) => Some(match body.as_json()? {
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

					let mut authorizations = Vec::with_capacity(authorization_urls.len());

					for http_common::DeserializableUri(authorization_url) in authorization_urls {
						let authorization = self.post(authorization_url.clone(), None::<&()>).await.context("could not get authorization")?;

						self.logger.report_state("acme/authorization", &authorization_url, format_args!("{authorization:?}"));

						let (mut hasher, challenge_url) = match authorization {
							AuthorizationResponse::Pending { hasher, challenge_url } => (hasher, challenge_url),
							AuthorizationResponse::Valid => continue,
						};

						sha2::Digest::update(&mut hasher, b".");

						let jwk_thumbprint = {
							let mut hasher: sha2::Sha256 = sha2::Digest::new();
							let mut serializer = serde_json::Serializer::new(&mut hasher);
							serde::Serialize::serialize(&self.account_key.as_jwk(), &mut serializer).expect("cannot fail to serialize JWK");
							sha2::Digest::finalize(hasher)
						};

						let hasher = {
							let mut writer = base64::write::EncoderWriter::new(hasher, &JWS_BASE64_ENGINE);
							std::io::Write::write_all(&mut writer, &jwk_thumbprint).expect("cannot fail to base64-encode JWK hash");
							writer.finish().expect("cannot fail to base64-encode JWK hash")
						};

						let hash = sha2::Digest::finalize(hasher);
						let dns_txt_record_content = base64::Engine::encode(&JWS_BASE64_ENGINE, hash);

						authorizations.push(OrderPendingAuthorization {
							authorization_url,
							challenge_url,
							dns_txt_record_content,
						});
					}

					break Order::Pending(OrderPending {
						order_url,
						authorizations,
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
			}

			order = self.post(order_url.clone(), None::<&()>).await.context("could not get order")?;
		};

		Ok(order)
	}

	pub async fn complete_authorization(
		&mut self,
		OrderPending {
			order_url,
			authorizations,
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
				status: http_common::StatusCode,
				body: Option<&mut http_common::ResponseBody<impl std::io::Read>>,
				headers: http_common::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				Ok(match (status, body) {
					(http_common::StatusCode::OK, Some(body)) => Some(match body.as_json()? {
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
				status: http_common::StatusCode,
				body: Option<&mut http_common::ResponseBody<impl std::io::Read>>,
				headers: http_common::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				Ok(match (status, body) {
					(http_common::StatusCode::OK, Some(body)) => Some(match body.as_json()? {
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

		for OrderPendingAuthorization {
			authorization_url,
			challenge_url,
			dns_txt_record_content: _,
		} in authorizations {
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
				}
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
				}
			}
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
			}
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
				status: http_common::StatusCode,
				body: Option<&mut http_common::ResponseBody<impl std::io::Read>>,
				_headers: http_common::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				Ok(match (status, body) {
					(http_common::StatusCode::OK, Some(body)) => {
						let certificate = body.as_str("application/pem-certificate-chain")?.into_owned();
						Some(CertificateResponse(certificate))
					},
					_ => None,
				})
			}
		}

		let certificate = self.logger.report_operation("acme/certificate", &certificate_url.clone(), <log2::ScopedObjectOperation>::Get, async {
			let CertificateResponse(certificate) = self.post(certificate_url, None::<&()>).await.context("could not download certificate")?;
			Ok::<_, anyhow::Error>(certificate)
		}).await?;

		Ok(certificate)
	}

	async fn post<TRequest, TResponse>(
		&mut self,
		url: http_common::Uri,
		body: Option<&TRequest>,
	) -> anyhow::Result<TResponse>
	where
		TRequest: serde::Serialize,
		TResponse: http_common::FromResponse,
	{
		// This fn encapsulates the non-generic parts of `post` to reduce code size from monomorphization.
		async fn make_request<K>(account: &mut Account<'_, K>, url: http_common::Uri, payload: Vec<u8>) -> anyhow::Result<http_common::Request<http_common::RequestBody>> where K: AccountKey {
			#[allow(clippy::declare_interior_mutable_const)] // Clippy doesn't like const http_common::HeaderValue
			const APPLICATION_JOSE_JSON: http_common::HeaderValue = http_common::HeaderValue::from_static("application/jose+json");

			let nonce =
				if let Some(nonce) = account.nonce.take() {
					nonce
				}
				else {
					struct NewNonceResponse;

					impl http_common::FromResponse for NewNonceResponse {
						fn from_response(
							status: http_common::StatusCode,
							_body: Option<&mut http_common::ResponseBody<impl std::io::Read>>,
							_headers: http_common::HeaderMap,
						) -> anyhow::Result<Option<Self>> {
							Ok(match status {
								http_common::StatusCode::OK => Some(NewNonceResponse),
								_ => None,
							})
						}
					}

					let log2::Secret(nonce) = account.logger.report_operation("acme/nonce", "", <log2::ScopedObjectOperation>::Get, async {
						let mut req = http_common::Request::new(Default::default());
						*req.method_mut() = http_common::Method::HEAD;
						*req.uri_mut() = account.new_nonce_url.clone();

						let ResponseWithNewNonce::<NewNonceResponse> { body: _, new_nonce } =
							account.inner.request(req).await
							.context("could not execute HTTP request")
							.context("newNonce URL did not return new nonce")?;

						let nonce = new_nonce.context("newNonce URL did not return new nonce")?;
						Ok::<_, anyhow::Error>(log2::Secret(nonce))
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
					nonce: &'a http_common::HeaderValue,

					url: std::fmt::Arguments<'a>,
				}

				#[derive(serde::Serialize)]
				enum JwkOrKid<'a> {
					#[serde(rename = "jwk")]
					Jwk(Jwk<'a>),

					#[serde(rename = "kid")]
					Kid(&'a str),
				}

				fn serialize_header_value<S>(header_value: &http_common::HeaderValue, serializer: S) -> Result<S::Ok, S::Error> where S: serde::Serializer {
					let header_value = header_value.to_str().map_err(serde::ser::Error::custom)?;
					serializer.serialize_str(header_value)
				}

				let jwk = account.account_key.as_jwk();
				let alg = jwk.crv.jws_sign_alg();

				let jwk_or_kid = account.account_url.as_deref().map_or_else(|| JwkOrKid::Jwk(jwk), JwkOrKid::Kid);

				let mut writer = base64::write::EncoderWriter::new(Vec::with_capacity(1024), &JWS_BASE64_ENGINE);
				let mut serializer = serde_json::Serializer::new(&mut writer);
				serde::Serialize::serialize(
					&Protected {
						alg,
						jwk_or_kid,
						nonce: &nonce,
						url: format_args!("{url}"),
					},
					&mut serializer,
				).context("could not serialize `protected`")?;
				writer.finish().expect("cannot fail to write to Vec<u8>")
			};

			let signature =
				account.account_key.sign([
					&protected,
					&b"."[..],
					&payload,
				]).await?;

			// All strings are base64 so there's no need to get serde_json involved
			let body = {
				const PART1: http_common::Bytes = http_common::Bytes::from_static(br#"{"payload":""#);
				const PART2: http_common::Bytes = http_common::Bytes::from_static(br#"","protected":""#);
				const PART3: http_common::Bytes = http_common::Bytes::from_static(br#"","signature":""#);
				const PART4: http_common::Bytes = http_common::Bytes::from_static(br#""}"#);

				let body = [
					PART1,
					payload.into(),
					PART2,
					protected.into(),
					PART3,
					signature.into(),
					PART4,
				];

				http_common::RequestBody::from_iter(body)
			};

			let mut req = http_common::Request::new(body);
			*req.method_mut() = http_common::Method::POST;
			*req.uri_mut() = url;
			req.headers_mut().insert(http_common::CONTENT_TYPE, APPLICATION_JOSE_JSON);
			Ok(req)
		}

		let payload =
			if let Some(payload) = body {
				let mut writer = base64::write::EncoderWriter::new(vec![], &JWS_BASE64_ENGINE);
				let mut serializer = serde_json::Serializer::new(&mut writer);
				serde::Serialize::serialize(payload, &mut serializer).context("could not serialize `payload`")?;
				writer.finish().expect("cannot fail to write to Vec<u8>")
			}
			else {
				vec![]
			};

		let req = make_request(self, url, payload).await?;

		let ResponseWithNewNonce { body, new_nonce } =
			self.inner.request(req).await.context("could not execute HTTP request")?;

		self.nonce = new_nonce;

		Ok(body)
	}
}

pub trait AccountKey {
	fn as_jwk(&self) -> Jwk<'_>;

	fn sign<I>(&self, digest: I) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<String>> + '_>>
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
	order_url: http_common::Uri,
	pub authorizations: Vec<OrderPendingAuthorization>,
}

pub struct OrderPendingAuthorization {
	authorization_url: http_common::Uri,
	challenge_url: http_common::Uri,
	pub dns_txt_record_content: String,
}

pub struct OrderReady {
	order_url: http_common::Uri,
}

pub struct OrderValid {
	certificate_url: http_common::Uri,
}

pub const JWS_BASE64_ENGINE: base64::engine::GeneralPurpose =
	base64::engine::GeneralPurpose::new(
		&base64::alphabet::URL_SAFE,
		base64::engine::general_purpose::NO_PAD,
	);

struct ResponseWithNewNonce<TResponse> {
	body: TResponse,
	new_nonce: Option<http_common::HeaderValue>,
}

impl<TResponse> http_common::FromResponse for ResponseWithNewNonce<TResponse> where TResponse: http_common::FromResponse {
	fn from_response(
		status: http_common::StatusCode,
		body: Option<&mut http_common::ResponseBody<impl std::io::Read>>,
		mut headers: http_common::HeaderMap,
	) -> anyhow::Result<Option<Self>> {
		#[allow(clippy::declare_interior_mutable_const)] // Clippy doesn't like const http_common::HeaderName
		const REPLAY_NONCE: http_common::HeaderName = http_common::HeaderName::from_static("replay-nonce");

		#[allow(clippy::borrow_interior_mutable_const)] // Clippy doesn't like const http_common::HeaderName
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
		certificate_url: http_common::Uri,
	},
}

impl<TPending, TReady> http_common::FromResponse for OrderResponse<TPending, TReady>
where
	TPending: serde::de::DeserializeOwned,
	TReady: serde::de::DeserializeOwned,
{
	fn from_response(
		status: http_common::StatusCode,
		body: Option<&mut http_common::ResponseBody<impl std::io::Read>>,
		headers: http_common::HeaderMap,
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
			(http_common::StatusCode::CREATED | http_common::StatusCode::OK, Some(body)) => Some(match body.as_json()? {
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
