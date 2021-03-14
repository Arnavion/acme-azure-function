impl<'a> super::Client<'a> {
	pub async fn csr_create(&self, certificate_name: &str, common_name: &str, key_type: CreateCsrKeyType) -> anyhow::Result<String> {
		#[derive(serde::Serialize)]
		struct Request<'a> {
			policy: RequestPolicy<'a>,
		}

		#[derive(serde::Serialize)]
		struct RequestPolicy<'a> {
			issuer: RequestPolicyIssuer<'a>,
			key_props: RequestPolicyKeyProps<'a>,
			x509_props: RequestPolicyX509Props<'a>,
		}

		#[derive(serde::Serialize)]
		struct RequestPolicyIssuer<'a> {
			cert_transparency: bool,
			name: &'a str,
		}

		#[derive(serde::Serialize)]
		struct RequestPolicyKeyProps<'a> {
			#[serde(skip_serializing_if = "Option::is_none")]
			crv: Option<super::EcCurve>,
			exportable: bool,
			#[serde(skip_serializing_if = "Option::is_none")]
			key_size: Option<u16>,
			kty: &'a str,
			reuse_key: bool,
		}

		#[derive(serde::Serialize)]
		struct RequestPolicyX509Props<'a> {
			sans: RequestPolicyX509PropsSans<'a>,
			subject: &'a str,
		}

		#[derive(serde::Serialize)]
		struct RequestPolicyX509PropsSans<'a> {
			dns_names: &'a [&'a str],
		}

		#[derive(serde::Deserialize)]
		struct Response {
			csr: String,
		}

		impl http_common::FromResponse for Response {
			fn from_response(
				status: http::StatusCode,
				body: Option<(&http::HeaderValue, &mut impl std::io::Read)>,
				_headers: http::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				Ok(match (status, body) {
					(http::StatusCode::ACCEPTED, Some((content_type, body))) if http_common::is_json(content_type) =>
						Some(serde_json::from_reader(body)?),
					_ => None,
				})
			}
		}

		let csr =
			self.logger.report_operation(
				"azure/key_vault/csr",
				(self.key_vault_name, certificate_name),
				log2::ScopedObjectOperation::Create { value: format_args!("{:?}", (common_name, key_type)) },
				async {
					let (url, authorization) = self.request_parameters(format_args!("/certificates/{}/create?api-version=7.1", certificate_name)).await?;

					let Response { csr } =
						self.client.request(
							http::Method::POST,
							url,
							authorization,
							Some(&Request {
								policy: RequestPolicy {
									issuer: RequestPolicyIssuer {
										cert_transparency: false,
										name: "Unknown",
									},
									key_props: {
										let (kty, crv, key_size, exportable) = match key_type {
											CreateCsrKeyType::Ec { curve, exportable } => ("EC", Some(curve), None, exportable),
											CreateCsrKeyType::EcHsm { curve } => ("EC-HSM", Some(curve), None, false),
											CreateCsrKeyType::Rsa { num_bits, exportable } => ("RSA", None, Some(num_bits), exportable),
											CreateCsrKeyType::RsaHsm { num_bits } => ("RSA-HSM", None, Some(num_bits), false),
										};

										RequestPolicyKeyProps {
											crv,
											exportable,
											key_size,
											kty,
											reuse_key: false,
										}
									},
									x509_props: RequestPolicyX509Props {
										sans: RequestPolicyX509PropsSans {
											dns_names: &[&common_name],
										},
										subject: &format!("CN={}", common_name),
									},
								},
							}),
						).await?;
					Ok::<_, anyhow::Error>(csr)
				},
			).await?;

		Ok(csr)
	}

	pub async fn certificate_get(&self, certificate_name: &str) -> anyhow::Result<Option<Certificate>> {
		struct Response(Option<Certificate>);

		impl http_common::FromResponse for Response {
			fn from_response(
				status: http::StatusCode,
				body: Option<(&http::HeaderValue, &mut impl std::io::Read)>,
				_headers: http::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				#[derive(serde::Deserialize)]
				struct ResponseInner {
					attributes: ResponseAttributes,
					id: String,
				}

				#[derive(serde::Deserialize)]
				struct ResponseAttributes {
					exp: i64,
				}

				Ok(match (status, body) {
					(http::StatusCode::OK, Some((content_type, body))) if http_common::is_json(content_type) => {
						let ResponseInner { attributes: ResponseAttributes { exp }, id } = serde_json::from_reader(body)?;

						let not_after =
							chrono::TimeZone::ymd(&chrono::Utc, 1970, 1, 1).and_hms(0, 0, 0) +
							chrono::Duration::seconds(exp);

						let version = id.split('/').last().expect("str::split yields at least one part").to_owned();

						Some(Response(Some(Certificate {
							version,
							not_after,
						})))
					},

					(http::StatusCode::NOT_FOUND, _) => Some(Response(None)),

					_ => None,
				})
			}
		}

		let certificate =
			self.logger.report_operation( "azure/key_vault/certificate", (self.key_vault_name, certificate_name), <log2::ScopedObjectOperation>::Get, async {
				let (url, authorization) = self.request_parameters(format_args!("/certificates/{}?api-version=7.1", certificate_name)).await?;

				let Response(certificate) =
					self.client.request(
						http::Method::GET,
						url,
						authorization,
						None::<&()>,
					).await?;
				Ok::<_, anyhow::Error>(certificate)
			}).await?;

		Ok(certificate)
	}

	pub async fn certificate_merge(&self, certificate_name: &str, certificates: &[String]) -> anyhow::Result<()> {
		#[derive(serde::Serialize)]
		struct Request<'a> {
			x5c: &'a [String],
		}

		struct Response;

		impl http_common::FromResponse for Response {
			fn from_response(
				status: http::StatusCode,
				_body: Option<(&http::HeaderValue, &mut impl std::io::Read)>,
				_headers: http::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				Ok(match status {
					http::StatusCode::CREATED => Some(Response),
					_ => None,
				})
			}
		}

		let () =
			self.logger.report_operation(
				"azure/key_vault/certificate",
				(self.key_vault_name, certificate_name),
				log2::ScopedObjectOperation::Create { value: format_args!("{:?}", certificates) },
				async {
					let (url, authorization) = self.request_parameters(format_args!("/certificates/{}/pending/merge?api-version=7.1", certificate_name)).await?;

					let _: Response =
						self.client.request(
							http::Method::POST,
							url,
							authorization,
							Some(&Request {
								x5c: certificates,
							}),
						).await?;
					Ok::<_, anyhow::Error>(())
				},
			).await?;

		Ok(())
	}
}

#[derive(Clone, Copy, Debug)]
pub enum CreateCsrKeyType {
	Ec {
		curve: super::EcCurve,
		exportable: bool,
	},

	EcHsm {
		curve: super::EcCurve,
	},

	Rsa {
		num_bits: u16,
		exportable: bool,
	},

	RsaHsm {
		num_bits: u16,
	},
}

#[derive(Debug)]
pub struct Certificate {
	pub version: String,
	pub not_after: chrono::DateTime<chrono::Utc>,
}
