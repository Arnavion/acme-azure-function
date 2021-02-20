use anyhow::Context;

pub(crate) struct Account<'a> {
	account_key: &'a azure::KeyVaultKey<'a>,
	account_url: String,
	client: http_common::Client,
	nonce: hyper::header::HeaderValue,
	new_order_url: String,
}

impl<'a> Account<'a> {
	pub(crate) async fn new(
		acme_directory_url: &'a str,
		acme_contact_url: &'a str,
		account_key: &'a azure::KeyVaultKey<'a>,
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
					_headers: hyper::HeaderMap,
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
				&client,
				&mut nonce,
				acme_directory_url,
			).await.context("could not query ACME directory")?;

			eprintln!("Got directory {}", acme_directory_url);

			(nonce, new_nonce_url, new_account_url, new_order_url)
		};

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
						_headers: hyper::HeaderMap,
					) -> anyhow::Result<Option<Self>> {
						Ok(match status {
							hyper::StatusCode::NO_CONTENT => Some(NewNonceResponse),
							_ => None,
						})
					}
				}

				eprintln!("Getting initial nonce...");

				let NewNonceResponse = get(
					&client,
					&mut nonce,
					&new_nonce_url,
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
					_headers: hyper::HeaderMap,
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

			let http_common::ResponseWithLocation {
				body: NewAccountResponse { status },
				location: account_url,
			} = post(
				account_key,
				None,
				&client,
				&mut nonce,
				&new_account_url,
				Some(&NewAccountRequest {
					contact_urls: &[acme_contact_url],
					terms_of_service_agreed: true,
				}),
			).await.context("could not create / get account")?;

			eprintln!("Created / got account {} with status {}", account_url, status);

			if status != "valid" {
				return Err(anyhow::anyhow!("Account has {} status", status));
			}

			account_url
		};

		let result = Account {
			client,
			account_key,
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

		let http_common::ResponseWithLocation::<OrderResponse> {
			location: order_url,
			body: _,
		} = post(
			self.account_key,
			Some(&self.account_url),
			&self.client,
			&mut self.nonce,
			&self.new_order_url,
			Some(&NewOrderRequest {
				identifiers: &[NewOrderRequestIdentifier {
					r#type: "dns",
					value: domain_name,
				}],
			}),
		).await.context("could not create / get order")?;

		eprintln!("Created order for {} : {}", domain_name, order_url);

		let order = loop {
			let order =
				post(
					self.account_key,
					Some(&self.account_url),
					&self.client,
					&mut self.nonce,
					&order_url,
					None::<&()>,
				).await.context("could not get order")?;

			eprintln!("Order {} is {:?}", order_url, order);

			match order {
				OrderResponse::Invalid { .. } => return Err(anyhow::anyhow!("order failed")),

				OrderResponse::Pending { mut authorization_urls } => {
					let authorization_url = authorization_urls.pop().context("no authorizations")?;
					if !authorization_urls.is_empty() {
						return Err(anyhow::anyhow!("more than one authorization"));
					}

					let authorization =
						post(
							self.account_key,
							Some(&self.account_url),
							&self.client,
							&mut self.nonce,
							&authorization_url,
							None::<&()>,
						).await.context("could not get authorization")?;

					eprintln!("Authorization {} is {:?}", authorization_url, authorization);

					let challenges = match authorization {
						AuthorizationResponse::Pending { challenges, retry_after: _ } => challenges,
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
							sha2::Digest::update(&mut hasher, self.account_key.jwk().thumbprint().as_bytes());
							let result = sha2::Digest::finalize(hasher);
							http_common::jws_base64_encode(&result)
						},
					});
				},

				OrderResponse::Processing { retry_after } => {
					eprintln!("Waiting for {:?} before rechecking order...", retry_after);
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

		let _: ChallengeResponse =
			post(
				self.account_key,
				Some(&self.account_url),
				&self.client,
				&mut self.nonce,
				&challenge_url,
				Some(&ChallengeCompleteRequest { }),
			).await.context("could not complete challenge")?;

		loop {
			let challenge =
				post(
					self.account_key,
					Some(&self.account_url),
					&self.client,
					&mut self.nonce,
					&challenge_url,
					None::<&()>,
				).await.context("could not get challenge")?;

			eprintln!("Challenge {} is {:?}", challenge_url, challenge);

			match challenge {
				ChallengeResponse::Pending { .. } => {
					let retry_after = std::time::Duration::from_secs(1);
					eprintln!("Waiting for {:?} before rechecking challenge...", retry_after);
					tokio::time::sleep(retry_after).await;
				},

				ChallengeResponse::Processing { retry_after } => {
					eprintln!("Waiting for {:?} before rechecking challenge...", retry_after);
					tokio::time::sleep(retry_after).await;
				},

				ChallengeResponse::Valid => break,

				_ => return Err(anyhow::anyhow!("challenge has unexpected status")),
			};
		}

		eprintln!("Waiting for authorization {} ...", authorization_url);

		loop {
			let authorization =
				post(
					self.account_key,
					Some(&self.account_url),
					&self.client,
					&mut self.nonce,
					&authorization_url,
					None::<&()>,
				).await.context("could not get authorization")?;

			eprintln!("Authorization {} is {:?}", authorization_url, authorization);

			match authorization {
				AuthorizationResponse::Pending { challenges: _, retry_after } => {
					eprintln!("Waiting for {:?} before rechecking authorization...", retry_after);
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
			let order =
				post(
					self.account_key,
					Some(&self.account_url),
					&self.client,
					&mut self.nonce,
					&order_url,
					None::<&()>,
				).await.context("could not get order")?;

			eprintln!("Order {} is {:?}", order_url, order);

			match order {
				OrderResponse::Invalid { .. } => return Err(anyhow::anyhow!("order failed")),

				OrderResponse::Pending { .. } => return Err(anyhow::anyhow!("order is still pending")),

				OrderResponse::Processing { retry_after } => {
					eprintln!("Waiting for {:?} before rechecking order...", retry_after);
					tokio::time::sleep(retry_after).await;
				},

				OrderResponse::Ready { finalize_url } => {
					let csr = http_common::jws_base64_encode(csr);

					let _: OrderResponse =
						post(
							self.account_key,
							Some(&self.account_url),
							&self.client,
							&mut self.nonce,
							&finalize_url,
							Some(&FinalizeOrderRequest {
								csr: &csr,
							}),
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
				_headers: hyper::HeaderMap,
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

		let CertificateResponse(certificate) =
			post(
				self.account_key,
				Some(&self.account_url),
				&self.client,
				&mut self.nonce,
				&certificate_url,
				None::<&()>,
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
	client: &http_common::Client,
	nonce: &mut Option<hyper::header::HeaderValue>,
	url: &str,
) -> anyhow::Result<TResponse>
where
	TResponse: http_common::FromResponse,
{
	let mut req = hyper::Request::new(Default::default());
	*req.method_mut() = hyper::Method::GET;
	*req.uri_mut() = url.parse().context("could not parse request URI")?;

	let ResponseEx { body, new_nonce } =
		client.request_inner(req).await.context("could not execute HTTP request")?;

	if let Some(new_nonce) = new_nonce {
		*nonce = Some(new_nonce);
	}

	Ok(body)
}

async fn post<TRequest, TResponse>(
	account_key: &azure::KeyVaultKey<'_>,
	account_url: Option<&str>,
	client: &http_common::Client,
	nonce: &mut hyper::header::HeaderValue,
	url: &str,
	body: Option<TRequest>,
) -> anyhow::Result<TResponse>
where
	TRequest: serde::Serialize,
	TResponse: http_common::FromResponse,
{
	#[derive(serde::Serialize)]
	struct Protected<'a> {
		alg: &'a str,

		#[serde(skip_serializing_if = "Option::is_none")]
		jwk: Option<azure::Jwk<'a>>,

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

	let mut req = {
		let body = account_key.jws(body, |alg| {
			let (jwk, kid) =
				account_url.map_or_else(
					|| (Some(account_key.jwk()), None),
					|account_url| (None, Some(account_url)),
				);
			let protected = Protected {
				alg,
				jwk,
				kid,
				nonce,
				url,
			};
			let protected = serde_json::to_vec(&protected).expect("could not serialize `protected`");
			protected
		}).await?;
		hyper::Request::new(body.into())
	};
	*req.method_mut() = hyper::Method::POST;
	*req.uri_mut() = url.parse().context("could not parse request URI")?;
	req.headers_mut().insert(hyper::header::CONTENT_TYPE, APPLICATION_JOSE_JSON.clone());

	let ResponseEx { body, new_nonce } =
		client.request_inner(req).await.context("could not execute HTTP request")?;

	*nonce = new_nonce.context("server did not return new nonce")?;

	Ok(body)
}

static APPLICATION_JOSE_JSON: once_cell::sync::Lazy<hyper::header::HeaderValue> =
	once_cell::sync::Lazy::new(|| hyper::header::HeaderValue::from_static("application/jose+json"));

struct ResponseEx<TResponse> {
	body: TResponse,
	new_nonce: Option<hyper::header::HeaderValue>,
}

impl<TResponse> http_common::FromResponse for ResponseEx<TResponse> where TResponse: http_common::FromResponse {
	fn from_response(
		status: hyper::StatusCode,
		body: Option<(&hyper::header::HeaderValue, &mut impl std::io::Read)>,
		mut headers: hyper::HeaderMap,
	) -> anyhow::Result<Option<Self>> {
		let new_nonce = headers.remove("replay-nonce");
		match TResponse::from_response(status, body, headers) {
			Ok(Some(body)) => Ok(Some(ResponseEx { body, new_nonce })),
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
		authorization_urls: Vec<String>,
	},

	#[serde(rename = "processing")]
	Processing {
		#[serde(default, skip)]
		retry_after: std::time::Duration,
	},

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
		headers: hyper::HeaderMap,
	) -> anyhow::Result<Option<Self>> {
		Ok(match (status, body) {
			(hyper::StatusCode::CREATED, Some((content_type, body))) |
			(hyper::StatusCode::OK, Some((content_type, body))) if http_common::is_json(content_type) => {
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
	Processing {
		#[serde(default, skip)]
		retry_after: std::time::Duration,
	},

	#[serde(rename = "valid")]
	Valid,
}

impl http_common::FromResponse for AuthorizationResponse {
	fn from_response(
		status: hyper::StatusCode,
		body: Option<(&hyper::header::HeaderValue, &mut impl std::io::Read)>,
		headers: hyper::HeaderMap,
	) -> anyhow::Result<Option<Self>> {
		Ok(match (status, body) {
			(hyper::StatusCode::OK, Some((content_type, body))) if http_common::is_json(content_type) => {
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

impl http_common::FromResponse for ChallengeResponse {
	fn from_response(
		status: hyper::StatusCode,
		body: Option<(&hyper::header::HeaderValue, &mut impl std::io::Read)>,
		headers: hyper::HeaderMap,
	) -> anyhow::Result<Option<Self>> {
		Ok(match (status, body) {
			(hyper::StatusCode::OK, Some((content_type, body))) if http_common::is_json(content_type) => {
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
