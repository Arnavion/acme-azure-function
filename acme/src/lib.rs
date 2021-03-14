#![deny(rust_2018_idioms, warnings)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(
	clippy::default_trait_access,
	clippy::let_unit_value,
	clippy::missing_errors_doc,
	clippy::too_many_lines,
)]

use anyhow::Context;

pub struct Account<'a, K> {
	account_key: &'a K,
	account_url: String,
	client: http_common::Client,
	nonce: http::HeaderValue,
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
		let client = http_common::Client::new(user_agent).context("could not create HTTP client")?;

		let mut nonce = None;

		let (new_nonce_url, new_account_url, new_order_url) = {
			#[derive(Debug, serde::Deserialize)]
			struct DirectoryResponse {
				#[serde(deserialize_with = "http_common::deserialize_http_uri")]
				#[serde(rename = "newAccount")]
				new_account_url: http::Uri,

				#[serde(deserialize_with = "http_common::deserialize_http_uri")]
				#[serde(rename = "newNonce")]
				new_nonce_url: http::Uri,

				#[serde(deserialize_with = "http_common::deserialize_http_uri")]
				#[serde(rename = "newOrder")]
				new_order_url: http::Uri,
			}

			impl http_common::FromResponse for DirectoryResponse {
				fn from_response(
					status: http::StatusCode,
					body: Option<(&http::HeaderValue, &mut impl std::io::Read)>,
					_headers: http::HeaderMap,
				) -> anyhow::Result<Option<Self>> {
					Ok(match (status, body) {
						(http::StatusCode::OK, Some((content_type, body))) if http_common::is_json(content_type) =>
							Some(serde_json::from_reader(body)?),
						_ => None,
					})
				}
			}

			let DirectoryResponse {
				new_account_url,
				new_nonce_url,
				new_order_url,
			} = logger.report_operation("acme/directory", &acme_directory_url.clone(), <log2::ScopedObjectOperation>::Get, async {
				get(
					&client,
					&mut nonce,
					acme_directory_url,
				).await.context("could not query ACME directory")
			}).await?;

			(new_nonce_url, new_account_url, new_order_url)
		};

		let mut nonce =
			if let Some(nonce) = nonce {
				logger.report_state("acme/nonce", "", "already have initial");
				nonce
			}
			else {
				struct NewNonceResponse;

				impl http_common::FromResponse for NewNonceResponse {
					fn from_response(
						status: http::StatusCode,
						_body: Option<(&http::HeaderValue, &mut impl std::io::Read)>,
						_headers: http::HeaderMap,
					) -> anyhow::Result<Option<Self>> {
						Ok(match status {
							http::StatusCode::NO_CONTENT => Some(NewNonceResponse),
							_ => None,
						})
					}
				}

				let log2::Secret(nonce) = logger.report_operation("acme/nonce", "", <log2::ScopedObjectOperation>::Get, async {
					let NewNonceResponse = get(
						&client,
						&mut nonce,
						new_nonce_url,
					).await.context("could not get initial nonce")?;

					let nonce = nonce.context("server did not return initial nonce")?;
					Ok::<_, anyhow::Error>(log2::Secret(nonce))
				}).await?;
				nonce
			};

		let account_url = {
			#[derive(serde::Serialize)]
			struct NewAccountRequest<'a> {
				#[serde(rename = "contact")]
				contact_urls: &'a [&'a str],

				#[serde(rename = "termsOfServiceAgreed")]
				terms_of_service_agreed: bool
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
					body: Option<(&http::HeaderValue, &mut impl std::io::Read)>,
					_headers: http::HeaderMap,
				) -> anyhow::Result<Option<Self>> {
					Ok(match (status, body) {
						(http::StatusCode::OK, Some((content_type, body))) |
						(http::StatusCode::CREATED, Some((content_type, body))) if http_common::is_json(content_type) =>
							Some(serde_json::from_reader(body)?),
						_ => None,
					})
				}
			}

			let (account_url, status) = logger.report_operation("acme/account", "", <log2::ScopedObjectOperation>::Get, async {
				let http_common::ResponseWithLocation {
					body: NewAccountResponse { status },
					location: account_url,
				} = post(
					account_key,
					None,
					&client,
					&mut nonce,
					new_account_url,
					Some(&NewAccountRequest {
						contact_urls: &[acme_contact_url],
						terms_of_service_agreed: true,
					}),
				).await.context("could not create / get account")?;
				Ok::<_, anyhow::Error>((account_url.to_string(), status))
			}).await?;

			logger.report_state("acme/account", &account_url, format_args!("{:?}", status));

			if !matches!(status, AccountStatus::Valid) {
				return Err(anyhow::anyhow!("Account has {:?} status", status));
			}

			account_url
		};

		let result = Account {
			account_key,
			account_url,
			client,
			nonce,
			new_order_url,
			logger,
		};

		Ok(result)
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

		let order_url = self.logger.report_operation("acme/order", domain_name, <log2::ScopedObjectOperation>::Get, async {
			let http_common::ResponseWithLocation::<OrderResponse> {
				location: order_url,
				body: _,
			} = post(
				self.account_key,
				Some(&self.account_url),
				&self.client,
				&mut self.nonce,
				self.new_order_url.clone(),
				Some(&NewOrderRequest {
					identifiers: &[NewOrderRequestIdentifier {
						r#type: "dns",
						value: domain_name,
					}],
				}),
			).await.context("could not create / get order")?;
			Ok::<_, anyhow::Error>(order_url)
		}).await?;

		let order = loop {
			let order =
				post(
					self.account_key,
					Some(&self.account_url),
					&self.client,
					&mut self.nonce,
					order_url.clone(),
					None::<&()>,
				).await.context("could not get order")?;

			self.logger.report_state("acme/order", &order_url, format_args!("{:?}", order));

			match order {
				OrderResponse::Invalid { .. } => return Err(anyhow::anyhow!("order failed")),

				OrderResponse::Pending { mut authorization_urls } => {
					let http_common::DeserializableUri(authorization_url) = authorization_urls.pop().context("no authorizations")?;
					if !authorization_urls.is_empty() {
						return Err(anyhow::anyhow!("more than one authorization"));
					}

					let authorization =
						post(
							self.account_key,
							Some(&self.account_url),
							&self.client,
							&mut self.nonce,
							authorization_url.clone(),
							None::<&()>,
						).await.context("could not get authorization")?;

					self.logger.report_state("acme/authorization", &authorization_url, format_args!("{:?}", authorization));

					let challenges = match authorization {
						AuthorizationResponse::Pending { challenges, retry_after: _ } => challenges,
						_ => return Err(anyhow::anyhow!("authorization has unexpected status")),
					};

					let (token, challenge_url) =
						challenges.into_iter()
						.find_map(|challenge| match challenge {
							ChallengeResponse::Pending { token: log2::Secret(token), r#type, url } => (r#type == "dns-01").then(|| (token, url)),
							_ => None,
						})
						.context("did not find any pending dns-01 challenges")?;

					let jwk_thumbprint = {
						let mut hasher: sha2::Sha256 = sha2::Digest::new();
						let mut serializer = serde_json::Serializer::new(&mut hasher);
						let () = serde::Serialize::serialize(&self.account_key.jwk(), &mut serializer).expect("cannot fail to serialize JWK");
						sha2::Digest::finalize(hasher)
					};

					let mut hasher: sha2::Sha256 = sha2::Digest::new();
					sha2::Digest::update(&mut hasher, token);
					sha2::Digest::update(&mut hasher, b".");

					{
						let mut writer = base64::write::EncoderWriter::new(&mut hasher, JWS_BASE64_CONFIG);
						let () = std::io::Write::write_all(&mut writer, &jwk_thumbprint).expect("cannot fail to base64-encode JWK hash");
						let _ = writer.finish().expect("cannot fail to base64-encode JWK hash");
					}

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
					self.logger.report_message(format_args!("Waiting for {:?} before rechecking order...", retry_after));
					tokio::time::sleep(retry_after).await;
				},

				OrderResponse::Ready { finalize_url: _ } => break Order::Ready(OrderReady {
					order_url,
				}),

				OrderResponse::Valid { certificate_url } => break Order::Valid(OrderValid {
					certificate_url,
				}),
			};
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

		self.logger.report_message(format_args!("Completing challenge {} ...", challenge_url));

		let _: ChallengeResponse =
			post(
				self.account_key,
				Some(&self.account_url),
				&self.client,
				&mut self.nonce,
				challenge_url.clone(),
				Some(&ChallengeCompleteRequest { }),
			).await.context("could not complete challenge")?;

		loop {
			let challenge =
				post(
					self.account_key,
					Some(&self.account_url),
					&self.client,
					&mut self.nonce,
					challenge_url.clone(),
					None::<&()>,
				).await.context("could not get challenge")?;

			self.logger.report_state("acme/challenge", &challenge_url, format_args!("{:?}", challenge));

			match challenge {
				ChallengeResponse::Pending { .. } => {
					let retry_after = std::time::Duration::from_secs(1);
					self.logger.report_message(format_args!("Waiting for {:?} before rechecking challenge...", retry_after));
					tokio::time::sleep(retry_after).await;
				},

				ChallengeResponse::Processing { retry_after } => {
					self.logger.report_message(format_args!("Waiting for {:?} before rechecking challenge...", retry_after));
					tokio::time::sleep(retry_after).await;
				},

				ChallengeResponse::Valid => break,

				_ => return Err(anyhow::anyhow!("challenge has unexpected status")),
			};
		}

		self.logger.report_message(format_args!("Waiting for authorization {} ...", authorization_url));

		loop {
			let authorization =
				post(
					self.account_key,
					Some(&self.account_url),
					&self.client,
					&mut self.nonce,
					authorization_url.clone(),
					None::<&()>,
				).await.context("could not get authorization")?;

			self.logger.report_state("acme/authorization", &authorization_url, format_args!("{:?}", authorization));

			match authorization {
				AuthorizationResponse::Pending { challenges: _, retry_after } => {
					self.logger.report_message(format_args!("Waiting for {:?} before rechecking authorization...", retry_after));
					tokio::time::sleep(retry_after).await;
				},

				AuthorizationResponse::Valid => break,

				_ => return Err(anyhow::anyhow!("authorization has unexpected status")),
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
		self.logger.report_message(format_args!("Finalizing order {} ...", order_url));

		let order = loop {
			let order =
				post(
					self.account_key,
					Some(&self.account_url),
					&self.client,
					&mut self.nonce,
					order_url.clone(),
					None::<&()>,
				).await.context("could not get order")?;

			self.logger.report_state("acme/order", &order_url, format_args!("{:?}", order));

			match order {
				OrderResponse::Invalid { .. } => return Err(anyhow::anyhow!("order failed")),

				OrderResponse::Pending { .. } => return Err(anyhow::anyhow!("order is still pending")),

				OrderResponse::Processing { retry_after } => {
					self.logger.report_message(format_args!("Waiting for {:?} before rechecking order...", retry_after));
					tokio::time::sleep(retry_after).await;
				},

				OrderResponse::Ready { finalize_url } => {
					#[derive(serde::Serialize)]
					struct FinalizeOrderRequest<'a> {
						csr: &'a str,
					}

					// libstd has no way to in-place replace some ASCII chars in a String with other ASCII chars.
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

					let _: OrderResponse =
						post(
							self.account_key,
							Some(&self.account_url),
							&self.client,
							&mut self.nonce,
							finalize_url,
							Some(&FinalizeOrderRequest {
								csr,
							}),
						).await.context("could not finalize order")?;
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
				body: Option<(&http::HeaderValue, &mut impl std::io::Read)>,
				_headers: http::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				Ok(match (status, body) {
					(http::StatusCode::OK, Some((content_type, body))) if content_type == "application/pem-certificate-chain" => {
						let mut certificate = String::new();
						let _ = std::io::Read::read_to_string(body, &mut certificate)?;
						Some(CertificateResponse(certificate))
					},
					_ => None,
				})
			}
		}

		let certificate = self.logger.report_operation("acme/certificate", &certificate_url.clone(), <log2::ScopedObjectOperation>::Get, async {
			let CertificateResponse(certificate) =
				post(
					self.account_key,
					Some(&self.account_url),
					&self.client,
					&mut self.nonce,
					certificate_url,
					None::<&()>,
				).await.context("could not download certificate")?;
			Ok::<_, anyhow::Error>(certificate)
		}).await?;

		Ok(certificate)
	}
}

pub trait AccountKey {
	fn jwk(&self) -> Jwk<'_>;

	fn sign<'a>(
		&'a self,
		alg: &'static str,
		digest: &'a str,
	) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<String>> + 'a>>;
}

#[derive(Clone, Copy, serde::Serialize)]
pub struct Jwk<'a> {
	pub crv: EcCurve,
	pub kty: &'a str,
	pub x: &'a str,
	pub y: &'a str,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
pub enum EcCurve {
	#[serde(rename = "P-256")]
	P256,

	#[serde(rename = "P-384")]
	P384,

	#[serde(rename = "P-521")]
	P521,
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

async fn get<TResponse>(
	client: &http_common::Client,
	nonce: &mut Option<http::HeaderValue>,
	url: http::Uri,
) -> anyhow::Result<TResponse>
where
	TResponse: http_common::FromResponse,
{
	let mut req = http::Request::new(Default::default());
	*req.method_mut() = http::Method::GET;
	*req.uri_mut() = url;

	let ResponseWithNewNonce { body, new_nonce } =
		client.request_inner(req).await.context("could not execute HTTP request")?;

	if let Some(new_nonce) = new_nonce {
		*nonce = Some(new_nonce);
	}

	Ok(body)
}

async fn post<TRequest, TResponse>(
	account_key: &impl AccountKey,
	account_url: Option<&str>,
	client: &http_common::Client,
	nonce: &mut http::HeaderValue,
	url: http::Uri,
	body: Option<&TRequest>,
) -> anyhow::Result<TResponse>
where
	TRequest: serde::Serialize,
	TResponse: http_common::FromResponse,
{
	fn serialize_header_value<S>(header_value: &http::HeaderValue, serializer: S) -> Result<S::Ok, S::Error> where S: serde::Serializer {
		let header_value = header_value.to_str().map_err(serde::ser::Error::custom)?;
		serializer.serialize_str(header_value)
	}

	static APPLICATION_JOSE_JSON: once_cell2::race::LazyBox<http::HeaderValue> =
		once_cell2::race::LazyBox::new(|| http::HeaderValue::from_static("application/jose+json"));

	let mut req = {
		#[derive(serde::Serialize)]
		struct Protected<'a> {
			alg: &'a str,

			#[serde(skip_serializing_if = "Option::is_none")]
			jwk: Option<Jwk<'a>>,

			#[serde(skip_serializing_if = "Option::is_none")]
			kid: Option<&'a str>,

			#[serde(serialize_with = "serialize_header_value")]
			nonce: &'a http::HeaderValue,

			url: std::fmt::Arguments<'a>,
		}

		#[derive(serde::Serialize)]
		struct Request<'a> {
			payload: &'a str,
			protected: &'a str,
			signature: &'a str,
		}

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
							base64::encode_config(&hash, JWS_BASE64_CONFIG)
						},
					)*
				}
			};
		}

		let jwk = account_key.jwk();

		let alg = match jwk.crv {
			EcCurve::P256 => "ES256",
			EcCurve::P384 => "ES384",
			EcCurve::P521 => "ES512",
		};

		let protected = {
			let (jwk, kid) = account_url.map_or_else(|| (Some(jwk), None), |account_url| (None, Some(account_url)));

			let mut writer = base64::write::EncoderStringWriter::new(JWS_BASE64_CONFIG);
			let mut serializer = serde_json::Serializer::new(&mut writer);
			let () =
				serde::Serialize::serialize(
					&Protected {
						alg,
						jwk,
						kid,
						nonce,
						url: format_args!("{}", url),
					},
					&mut serializer,
				).context("could not serialize `protected`")?;
			writer.into_inner()
		};

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

		let digest = hash!(jwk.crv, &protected, &payload, {
			EcCurve::P256 => sha2::Sha256,
			EcCurve::P384 => sha2::Sha384,
			EcCurve::P521 => sha2::Sha512,
		});
		let signature = account_key.sign(alg, &digest).await?;

		let body = Request {
			payload: &payload,
			protected: &protected,
			signature: &signature,
		};
		let body = serde_json::to_vec(&body).expect("could not serialize JWS request body");
		http::Request::new(body.into())
	};
	*req.method_mut() = http::Method::POST;
	*req.uri_mut() = url;
	req.headers_mut().insert(http::header::CONTENT_TYPE, APPLICATION_JOSE_JSON.clone());

	let ResponseWithNewNonce { body, new_nonce } =
		client.request_inner(req).await.context("could not execute HTTP request")?;

	*nonce = new_nonce.context("server did not return new nonce")?;

	Ok(body)
}

const JWS_BASE64_CONFIG: base64::Config = base64::Config::new(base64::CharacterSet::UrlSafe, false);

struct ResponseWithNewNonce<TResponse> {
	body: TResponse,
	new_nonce: Option<http::HeaderValue>,
}

impl<TResponse> http_common::FromResponse for ResponseWithNewNonce<TResponse> where TResponse: http_common::FromResponse {
	fn from_response(
		status: http::StatusCode,
		body: Option<(&http::HeaderValue, &mut impl std::io::Read)>,
		mut headers: http::HeaderMap,
	) -> anyhow::Result<Option<Self>> {
		static REPLAY_NONCE: once_cell2::race::LazyBox<http::header::HeaderName> =
			once_cell2::race::LazyBox::new(|| http::header::HeaderName::from_static("replay-nonce"));

		let new_nonce = headers.remove(&*REPLAY_NONCE);
		match TResponse::from_response(status, body, headers) {
			Ok(Some(body)) => Ok(Some(ResponseWithNewNonce { body, new_nonce })),
			Ok(None) => Ok(None),
			Err(err) => Err(err),
		}
	}
}

#[derive(Debug, serde::Deserialize)]
#[serde(tag = "status")]
enum OrderResponse {
	#[serde(rename = "invalid")]
	Invalid {
		#[serde(flatten)]
		body: serde_json::Value,
	},

	#[serde(rename = "pending")]
	Pending {
		#[serde(rename = "authorizations")]
		authorization_urls: Vec<http_common::DeserializableUri>,
	},

	#[serde(rename = "processing")]
	Processing {
		#[serde(default, skip)]
		retry_after: std::time::Duration,
	},

	#[serde(rename = "ready")]
	Ready {
		#[serde(deserialize_with = "http_common::deserialize_http_uri")]
		#[serde(rename = "finalize")]
		finalize_url: http::Uri,
	},

	#[serde(rename = "valid")]
	Valid {
		#[serde(deserialize_with = "http_common::deserialize_http_uri")]
		#[serde(rename = "certificate")]
		certificate_url: http::Uri,
	},
}

impl http_common::FromResponse for OrderResponse {
	fn from_response(
		status: http::StatusCode,
		body: Option<(&http::HeaderValue, &mut impl std::io::Read)>,
		headers: http::HeaderMap,
	) -> anyhow::Result<Option<Self>> {
		Ok(match (status, body) {
			(http::StatusCode::CREATED, Some((content_type, body))) |
			(http::StatusCode::OK, Some((content_type, body))) if http_common::is_json(content_type) => {
				let mut body = serde_json::from_reader(body)?;
				if let OrderResponse::Processing { retry_after } = &mut body {
					*retry_after = http_common::get_retry_after(&headers, std::time::Duration::from_secs(1), std::time::Duration::from_secs(30))?;
				}
				Some(body)
			},
			_ => None,
		})
	}
}

#[derive(Debug, serde::Deserialize)]
#[serde(tag = "status")]
enum AuthorizationResponse {
	#[serde(rename = "deactivated")]
	Deactivated,

	#[serde(rename = "expired")]
	Expired,

	#[serde(rename = "invalid")]
	Invalid,

	#[serde(rename = "pending")]
	Pending {
		challenges: Vec<ChallengeResponse>,

		#[serde(default, skip)]
		retry_after: std::time::Duration,
	},

	#[serde(rename = "revoked")]
	Revoked,

	#[serde(rename = "valid")]
	Valid,
}

impl http_common::FromResponse for AuthorizationResponse {
	fn from_response(
		status: http::StatusCode,
		body: Option<(&http::HeaderValue, &mut impl std::io::Read)>,
		headers: http::HeaderMap,
	) -> anyhow::Result<Option<Self>> {
		Ok(match (status, body) {
			(http::StatusCode::OK, Some((content_type, body))) if http_common::is_json(content_type) => {
				let mut body = serde_json::from_reader(body)?;
				if let AuthorizationResponse::Pending { challenges: _, retry_after } = &mut body {
					*retry_after = http_common::get_retry_after(&headers, std::time::Duration::from_secs(1), std::time::Duration::from_secs(30))?;
				}
				Some(body)
			},
			_ => None,
		})
	}
}

#[derive(Debug, serde::Deserialize)]
#[serde(tag = "status")]
enum ChallengeResponse {
	#[serde(rename = "invalid")]
	Invalid {
		#[serde(flatten)]
		body: serde_json::Value,
	},

	#[serde(rename = "pending")]
	Pending {
		token: log2::Secret<String>,
		r#type: String,
		#[serde(deserialize_with = "http_common::deserialize_http_uri")]
		url: http::Uri,
	},

	#[serde(rename = "processing")]
	Processing {
		#[serde(default, skip)]
		retry_after: std::time::Duration,
	},

	#[serde(rename = "valid")]
	Valid,
}

impl http_common::FromResponse for ChallengeResponse {
	fn from_response(
		status: http::StatusCode,
		body: Option<(&http::HeaderValue, &mut impl std::io::Read)>,
		headers: http::HeaderMap,
	) -> anyhow::Result<Option<Self>> {
		Ok(match (status, body) {
			(http::StatusCode::OK, Some((content_type, body))) if http_common::is_json(content_type) => {
				let mut body = serde_json::from_reader(body)?;
				if let ChallengeResponse::Processing { retry_after } = &mut body {
					*retry_after = http_common::get_retry_after(&headers, std::time::Duration::from_secs(1), std::time::Duration::from_secs(30))?;
				}
				Some(body)
			},
			_ => None,
		})
	}
}
