use anyhow::Context;

pub(crate) struct Account<'a> {
	pub(crate) azure_account: &'a mut azure::Account<'a>,

	jwk_thumbprint: String,
	account_key_kid: &'a str,
	account_jws_alg: &'static str,

	client: http_common::Client,
	account_url: String,
	nonce: hyper::header::HeaderValue,
	new_order_url: String,
}

impl<'a> Account<'a> {
	pub(crate) async fn new(
		acme_directory_url: &'a str,
		acme_contact_url: &'a str,
		azure_account: &'a mut azure::Account<'a>,
		account_key: &'a azure::KeyVaultKey,
		user_agent: &str,
	) -> anyhow::Result<Account<'a>> {
		let client = http_common::Client::new(user_agent).context("could not create HTTP client")?;

		let (mut nonce, new_nonce_url, new_account_url, new_order_url) = {
			#[derive(serde::Deserialize)]
			struct DirectoryResponse {
				#[serde(rename = "newAccount")]
				new_account_url: String,

				#[serde(rename = "newNonce")]
				new_nonce_url: String,

				#[serde(rename = "newOrder")]
				new_order_url: String,
			}

			impl http_common::FromResponse for DirectoryResponse {
				fn from_response(
					status: hyper::StatusCode,
					body: Option<(&hyper::header::HeaderValue, &mut impl std::io::Read)>,
				) -> anyhow::Result<Option<Self>> {
					Ok(match (status, body) {
						(hyper::StatusCode::OK, Some((content_type, body))) if http_common::is_json(content_type) =>
							Some(serde_json::from_reader(body)?),
						_ => None,
					})
				}
			}

			eprintln!("Getting directory {} ...", acme_directory_url);

			let mut nonce = None;

			let DirectoryResponse {
				new_account_url,
				new_nonce_url,
				new_order_url,
			} = get(
				acme_directory_url,
				&mut nonce,
				&client,
			).await.context("could not query ACME directory")?;

			eprintln!("Got directory {}", acme_directory_url);

			(nonce, new_nonce_url, new_account_url, new_order_url)
		};

		let jwk = Jwk {
			crv: &account_key.crv,
			kty: &account_key.kty,
			x: &account_key.x,
			y: &account_key.y,
		};
		let account_key_kid = &account_key.kid;
		let account_jws_alg = match &*account_key.crv {
			"P-384" => "ES384",
			crv => return Err(anyhow::anyhow!("unexpected account key curve {:?}", crv)),
		};

		eprintln!("Creating account key thumbprint...");
		let jwk_thumbprint = {
			let jwk = serde_json::to_vec(&jwk).context("could not serialize JWK")?;
			let mut hasher: sha2::Sha256 = sha2::Digest::new();
			sha2::Digest::update(&mut hasher, &jwk);
			let result = sha2::Digest::finalize(hasher);
			http_common::jws_base64_encode(&result)
		};
		eprintln!("Created account key thumbprint");

		let mut nonce =
			if let Some(nonce) = nonce {
				eprintln!("Already have initial nonce");
				nonce
			}
			else {
				struct NewNonceResponse;

				impl http_common::FromResponse for NewNonceResponse {
					fn from_response(
						status: hyper::StatusCode,
						_body: Option<(&hyper::header::HeaderValue, &mut impl std::io::Read)>,
					) -> anyhow::Result<Option<Self>> {
						Ok(match status {
							hyper::StatusCode::NO_CONTENT => Some(NewNonceResponse),
							_ => None,
						})
					}
				}

				eprintln!("Getting initial nonce...");

				let NewNonceResponse = get(
					&new_nonce_url,
					&mut nonce,
					&client,
				).await.context("could not get initial nonce")?;

				let nonce = nonce.context("server did not return initial nonce")?;

				eprintln!("Got initial nonce");
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
				status: String,
			}

			impl http_common::FromResponse for NewAccountResponse {
				fn from_response(
					status: hyper::StatusCode,
					body: Option<(&hyper::header::HeaderValue, &mut impl std::io::Read)>,
				) -> anyhow::Result<Option<Self>> {
					Ok(match (status, body) {
						(hyper::StatusCode::OK, Some((content_type, body))) |
						(hyper::StatusCode::CREATED, Some((content_type, body))) if http_common::is_json(content_type) =>
							Some(serde_json::from_reader(body)?),
						_ => None,
					})
				}
			}

			eprintln!("Creating / getting account corresponding to account key...");

			let Response {
				location,
				body: NewAccountResponse {
					status,
				},
			} = post(
				&new_account_url,
				Auth::Jwk(jwk),
				Some(&NewAccountRequest {
					contact_urls: &[acme_contact_url],
					terms_of_service_agreed: true,
				}),
				&mut nonce,
				azure_account,
				account_key_kid,
				account_jws_alg,
				&client,
			).await.context("coult not create / get account")?;

			let account_url = location.context("server did not return account URL from newAccount endpoint")?;

			eprintln!("Created / got account {} with status {}", account_url, status);

			if status != "valid" {
				return Err(anyhow::anyhow!("Account has {} status", status));
			}

			account_url
		};

		let result = Account {
			azure_account,

			jwk_thumbprint,
			account_key_kid,
			account_jws_alg,

			client,
			account_url,
			nonce,
			new_order_url,
		};

		Ok(result)
	}

	pub(crate) async fn place_order(&mut self, domain_name: &str) -> anyhow::Result<Order> {
		#[derive(serde::Serialize)]
		struct NewOrderRequest<'a> {
			identifiers: &'a [NewOrderRequestIdentifier<'a>],
		}

		#[derive(serde::Serialize)]
		struct NewOrderRequestIdentifier<'a> {
			r#type: &'a str,
			value: &'a str,
		}

		eprintln!("Creating order for {} ...", domain_name);

		let Response::<OrderResponse> {
			location: order_url,
			body: _,
		} = post(
			&self.new_order_url,
			Auth::AccountUrl(&self.account_url),
			Some(&NewOrderRequest {
				identifiers: &[NewOrderRequestIdentifier {
					r#type: "dns",
					value: domain_name,
				}],
			}),
			&mut self.nonce,
			&mut self.azure_account,
			self.account_key_kid,
			self.account_jws_alg,
			&self.client,
		).await.context("could not create / get order")?;

		let order_url = order_url.context("newOrder endpoint did not return order URL")?;

		eprintln!("Created order for {} : {}", domain_name, order_url);

		let order = loop {
			let Response {
				location: _,
				body: order,
			} = post(
				&order_url,
				Auth::AccountUrl(&self.account_url),
				None::<&()>,
				&mut self.nonce,
				&mut self.azure_account,
				self.account_key_kid,
				self.account_jws_alg,
				&self.client,
			).await.context("could not get order")?;

			eprintln!("Order {} is {:?}", order_url, order);

			match order {
				OrderResponse::Invalid { .. } => return Err(anyhow::anyhow!("order failed")),

				OrderResponse::Pending { mut authorization_urls } => {
					let authorization_url = authorization_urls.pop().context("no authorizations")?;
					if !authorization_urls.is_empty() {
						return Err(anyhow::anyhow!("more than one authorization"));
					}

					let Response {
						location: _,
						body: authorization,
					} = post(
						&authorization_url,
						Auth::AccountUrl(&self.account_url),
						None::<&()>,
						&mut self.nonce,
						&mut self.azure_account,
						self.account_key_kid,
						self.account_jws_alg,
						&self.client,
					).await.context("could not get authorization")?;

					eprintln!("Authorization {} is {:?}", authorization_url, authorization);

					let challenges = match authorization {
						AuthorizationResponse::Pending { challenges } => challenges,
						_ => return Err(anyhow::anyhow!("authorization has unexpected status")),
					};

					let (token, challenge_url) =
						challenges.into_iter()
						.find_map(|challenge| match challenge {
							ChallengeResponse::Pending { token, r#type, url } => (r#type == "dns-01").then(|| (token, url)),
							_ => None,
						})
						.context("did not find any pending dns-01 challenges")?;

					break Order::Pending(OrderPending {
						order_url,
						authorization_url,
						challenge_url,
						dns_txt_record_content: {
							let mut hasher: sha2::Sha256 = sha2::Digest::new();
							sha2::Digest::update(&mut hasher, token.as_bytes());
							sha2::Digest::update(&mut hasher, b".");
							sha2::Digest::update(&mut hasher, self.jwk_thumbprint.as_bytes());
							let result = sha2::Digest::finalize(hasher);
							http_common::jws_base64_encode(&result)
						},
					});
				},

				OrderResponse::Processing =>
					tokio::time::sleep(std::time::Duration::from_secs(1)).await,

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

	pub(crate) async fn complete_authorization(
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

		eprintln!("Completing challenge {} ...", challenge_url);

		let Response::<ChallengeResponse> {
			location: _,
			body: _,
		} = post(
			&challenge_url,
			Auth::AccountUrl(&self.account_url),
			Some(&ChallengeCompleteRequest { }),
			&mut self.nonce,
			&mut self.azure_account,
			self.account_key_kid,
			self.account_jws_alg,
			&self.client,
		).await.context("could not complete challenge")?;

		loop {
			let Response {
				location: _,
				body: challenge,
			} = post(
				&challenge_url,
				Auth::AccountUrl(&self.account_url),
				None::<&()>,
				&mut self.nonce,
				&mut self.azure_account,
				self.account_key_kid,
				self.account_jws_alg,
				&self.client,
			).await.context("could not get challenge")?;

			eprintln!("Challenge {} is {:?}", challenge_url, challenge);

			match challenge {
				ChallengeResponse::Pending { .. } |
				ChallengeResponse::Processing =>
					tokio::time::sleep(std::time::Duration::from_secs(1)).await,

				ChallengeResponse::Valid => break,

				_ => return Err(anyhow::anyhow!("challenge has unexpected status")),
			};
		}

		eprintln!("Waiting for authorization {} ...", authorization_url);

		loop {
			let Response {
				location: _,
				body: authorization,
			} = post(
				&authorization_url,
				Auth::AccountUrl(&self.account_url),
				None::<&()>,
				&mut self.nonce,
				&mut self.azure_account,
				self.account_key_kid,
				self.account_jws_alg,
				&self.client,
			).await.context("could not get authorization")?;

			eprintln!("Authorization {} is {:?}", authorization_url, authorization);

			match authorization {
				AuthorizationResponse::Pending { .. } =>
					tokio::time::sleep(std::time::Duration::from_secs(1)).await,

				AuthorizationResponse::Valid => break,

				_ => return Err(anyhow::anyhow!("authorization has unexpected status")),
			};
		}

		Ok(OrderReady {
			order_url,
		})
	}

	pub(crate) async fn finalize_order(
		&mut self,
		OrderReady {
			order_url,
		}: OrderReady,
		csr: &[u8],
	) -> anyhow::Result<OrderValid> {
		#[derive(serde::Serialize)]
		struct FinalizeOrderRequest<'a> {
			csr: &'a str,
		}

		eprintln!("Finalizing order {} ...", order_url);

		let order = loop {
			let Response {
				location: _,
				body: order,
			} = post(
				&order_url,
				Auth::AccountUrl(&self.account_url),
				None::<&()>,
				&mut self.nonce,
				&mut self.azure_account,
				self.account_key_kid,
				self.account_jws_alg,
				&self.client,
			).await.context("could not get order")?;

			eprintln!("Order {} is {:?}", order_url, order);

			match order {
				OrderResponse::Invalid { .. } => return Err(anyhow::anyhow!("order failed")),

				OrderResponse::Pending { .. } => return Err(anyhow::anyhow!("order is still pending")),

				OrderResponse::Processing =>
					tokio::time::sleep(std::time::Duration::from_secs(1)).await,

				OrderResponse::Ready { finalize_url } => {
					let csr = http_common::jws_base64_encode(csr);

					let Response::<OrderResponse> {
						location: _,
						body: _,
					} = post(
						&finalize_url,
						Auth::AccountUrl(&self.account_url),
						Some(&FinalizeOrderRequest {
							csr: &csr,
						}),
						&mut self.nonce,
						&mut self.azure_account,
						self.account_key_kid,
						self.account_jws_alg,
						&self.client,
					).await.context("could not finalize order")?;

					continue;
				},

				OrderResponse::Valid { certificate_url } => break OrderValid {
					certificate_url,
				},
			};
		};

		Ok(order)
	}

	pub(crate) async fn download_certificate(
		&mut self,
		OrderValid {
			certificate_url,
		}: OrderValid,
	) -> anyhow::Result<String> {
		struct CertificateResponse(String);

		impl http_common::FromResponse for CertificateResponse {
			fn from_response(
				status: hyper::StatusCode,
				body: Option<(&hyper::header::HeaderValue, &mut impl std::io::Read)>,
			) -> anyhow::Result<Option<Self>> {
				Ok(match (status, body) {
					(hyper::StatusCode::OK, Some((content_type, body))) if content_type == "application/pem-certificate-chain" => {
						let mut certificate = String::new();
						let _ = std::io::Read::read_to_string(body, &mut certificate)?;
						Some(CertificateResponse(certificate))
					},
					_ => None,
				})
			}
		}

		eprintln!("Downloading certificate {} ...", certificate_url);

		let Response {
			location: _,
			body: CertificateResponse(certificate),
		} = post(
			&certificate_url,
			Auth::AccountUrl(&self.account_url),
			None::<&()>,
			&mut self.nonce,
			&mut self.azure_account,
			self.account_key_kid,
			self.account_jws_alg,
			&self.client,
		).await.context("could not download certificate")?;

		eprintln!("Downloaded certificate {}", certificate_url);

		Ok(certificate)
	}
}

pub(crate) enum Order {
	Pending(OrderPending),
	Ready(OrderReady),
	Valid(OrderValid),
}

pub(crate) struct OrderPending {
	order_url: String,
	authorization_url: String,
	challenge_url: String,
	pub(crate) dns_txt_record_content: String,
}

pub(crate) struct OrderReady {
	order_url: String,
}

pub(crate) struct OrderValid {
	certificate_url: String,
}

async fn get<TResponse>(
	url: &str,
	nonce: &mut Option<hyper::header::HeaderValue>,
	client: &http_common::Client,
) -> anyhow::Result<TResponse>
where
	TResponse: http_common::FromResponse,
{
	let mut req = hyper::Request::new(Default::default());
	*req.method_mut() = hyper::Method::GET;
	*req.uri_mut() = url.parse().context("could not parse request URI")?;

	let (body, mut headers) = client.request_inner(req).await.context("could not execute HTTP request")?;

	if let Some(new_nonce) = headers.remove("Replay-Nonce") {
		*nonce = Some(new_nonce);
	}

	Ok(body)
}

async fn post<TRequest, TResponse>(
	url: &str,
	auth: Auth<'_>,
	body: Option<&TRequest>,
	nonce: &mut hyper::header::HeaderValue,
	azure_account: &mut azure::Account<'_>,
	account_key_kid: &str,
	account_jws_alg: &str,
	client: &http_common::Client,
) -> anyhow::Result<Response<TResponse>>
where
	TRequest: serde::Serialize,
	TResponse: http_common::FromResponse,
{
	let mut req = {
		let (kid, jwk) = match auth {
			Auth::AccountUrl(account_url) => (Some(account_url), None),
			Auth::Jwk(jwk) => (None, Some(jwk)),
		};

		let protected = Protected {
			alg: account_jws_alg,
			jwk,
			kid,
			nonce,
			url,
		};
		let protected_encoded = serde_json::to_vec(&protected).context("could not serialize `protected`")?;
		let protected_encoded = http_common::jws_base64_encode(&protected_encoded);

		let payload_encoded =
			if let Some(body) = body {
				let payload_encoded = serde_json::to_vec(body).context("could not serialize `payload`")?;
				let payload_encoded = http_common::jws_base64_encode(&payload_encoded);
				payload_encoded
			}
			else {
				String::new()
			};

		let signature_input = {
			let mut hasher: sha2::Sha384 = sha2::Digest::new();
			sha2::Digest::update(&mut hasher, &protected_encoded);
			sha2::Digest::update(&mut hasher, b".");
			sha2::Digest::update(&mut hasher, &payload_encoded);
			sha2::Digest::finalize(hasher)
		};
		let signature = azure_account.key_vault_key_sign(account_key_kid, protected.alg, &signature_input).await?;

		let body = Request {
			payload: &payload_encoded,
			protected: &protected_encoded,
			signature: &signature,
		};
		let body = serde_json::to_vec(&body).context("could not serialize request body")?;

		hyper::Request::new(body.into())
	};
	*req.method_mut() = hyper::Method::POST;
	*req.uri_mut() = url.parse().context("could not parse request URI")?;
	req.headers_mut().insert(hyper::header::CONTENT_TYPE, APPLICATION_JOSE_JSON.clone());

	let (body, mut headers) = client.request_inner(req).await.context("could not execute HTTP request")?;

	*nonce = headers.remove("Replay-Nonce").context("server did not return new nonce")?;

	let location = match headers.get(hyper::header::LOCATION) {
		Some(location) => {
			let location = location.to_str().with_context(|| format!("malformed location header value {:?}", location))?;
			Some(location.to_owned())
		},
		None => None,
	};

	Ok(Response {
		location,
		body,
	})
}

static APPLICATION_JOSE_JSON: once_cell::sync::Lazy<hyper::header::HeaderValue> =
	once_cell::sync::Lazy::new(|| hyper::header::HeaderValue::from_static("application/jose+json"));

enum Auth<'a> {
	AccountUrl(&'a str),
	Jwk(Jwk<'a>),
}

#[derive(serde::Serialize)]
struct Jwk<'a> {
	crv: &'a str,
	kty: &'a str,
	x: &'a str,
	y: &'a str,
}

#[derive(serde::Serialize)]
struct Request<'a> {
	payload: &'a str,
	protected: &'a str,
	signature: &'a str,
}

struct Response<B> {
	location: Option<String>,
	body: B,
}

#[derive(serde::Serialize)]
struct Protected<'a> {
	alg: &'a str,

	#[serde(skip_serializing_if = "Option::is_none")]
	jwk: Option<Jwk<'a>>,

	#[serde(skip_serializing_if = "Option::is_none")]
	kid: Option<&'a str>,

	#[serde(serialize_with = "serialize_header_value")]
	nonce: &'a hyper::header::HeaderValue,

	url: &'a str,
}

fn serialize_header_value<S>(header_value: &hyper::header::HeaderValue, serializer: S) -> Result<S::Ok, S::Error> where S: serde::Serializer {
	let header_value = header_value.to_str().map_err(serde::ser::Error::custom)?;
	serializer.serialize_str(header_value)
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
		authorization_urls: Vec<String>,
	},

	#[serde(rename = "processing")]
	Processing,

	#[serde(rename = "ready")]
	Ready {
		#[serde(rename = "finalize")]
		finalize_url: String,
	},

	#[serde(rename = "valid")]
	Valid {
		#[serde(rename = "certificate")]
		certificate_url: String,
	},
}

impl http_common::FromResponse for OrderResponse {
	fn from_response(
		status: hyper::StatusCode,
		body: Option<(&hyper::header::HeaderValue, &mut impl std::io::Read)>,
	) -> anyhow::Result<Option<Self>> {
		Ok(match (status, body) {
			(hyper::StatusCode::CREATED, Some((content_type, body))) |
			(hyper::StatusCode::OK, Some((content_type, body))) if http_common::is_json(content_type) =>
				Some(serde_json::from_reader(body)?),
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
	},

	#[serde(rename = "revoked")]
	Revoked,

	#[serde(rename = "valid")]
	Valid,
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
		token: String,
		r#type: String,
		url: String,
	},

	#[serde(rename = "processing")]
	Processing,

	#[serde(rename = "valid")]
	Valid,
}

impl http_common::FromResponse for AuthorizationResponse {
	fn from_response(
		status: hyper::StatusCode,
		body: Option<(&hyper::header::HeaderValue, &mut impl std::io::Read)>,
	) -> anyhow::Result<Option<Self>> {
		Ok(match (status, body) {
			(hyper::StatusCode::OK, Some((content_type, body))) if http_common::is_json(content_type) =>
				Some(serde_json::from_reader(body)?),
			_ => None,
		})
	}
}

impl http_common::FromResponse for ChallengeResponse {
	fn from_response(
		status: hyper::StatusCode,
		body: Option<(&hyper::header::HeaderValue, &mut impl std::io::Read)>,
	) -> anyhow::Result<Option<Self>> {
		Ok(match (status, body) {
			(hyper::StatusCode::OK, Some((content_type, body))) if http_common::is_json(content_type) =>
				Some(serde_json::from_reader(body)?),
			_ => None,
		})
	}
}
