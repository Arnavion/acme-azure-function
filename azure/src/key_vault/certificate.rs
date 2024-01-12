use anyhow::Context;

impl<'a> super::Client<'a> {
	pub async fn csr_create(&self, certificate_name: &str, common_name: &str, key_type: CreateCsrKeyType) -> anyhow::Result<String> {
		#[derive(serde::Serialize)]
		struct Request<'a> {
			policy: RequestPolicy<'a>,
		}

		#[derive(serde::Serialize)]
		struct RequestPolicy<'a> {
			issuer: RequestPolicyIssuer<'a>,
			key_props: RequestPolicyKeyProps,
			x509_props: RequestPolicyX509Props<'a>,
		}

		#[derive(serde::Serialize)]
		struct RequestPolicyIssuer<'a> {
			cert_transparency: bool,
			name: &'a str,
		}

		#[derive(serde::Serialize)]
		struct RequestPolicyKeyProps {
			#[serde(flatten)]
			key_type: CreateCsrKeyType,
			reuse_key: bool,
		}

		#[derive(serde::Serialize)]
		struct RequestPolicyX509Props<'a> {
			sans: RequestPolicyX509PropsSans<'a>,
			subject: std::fmt::Arguments<'a>,
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
				status: http_common::StatusCode,
				body: Option<&mut http_common::ResponseBody<impl std::io::Read>>,
				_headers: http_common::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				Ok(match (status, body) {
					(http_common::StatusCode::ACCEPTED, Some(body)) => Some(body.as_json()?),
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
					let Response { csr } =
						crate::request(
							self,
							http_common::Method::POST,
							format_args!("/certificates/{certificate_name}/create?api-version=7.3"),
							Some(&Request {
								policy: RequestPolicy {
									issuer: RequestPolicyIssuer {
										cert_transparency: false,
										name: "Unknown",
									},
									key_props: RequestPolicyKeyProps {
										key_type,
										reuse_key: false,
									},
									x509_props: RequestPolicyX509Props {
										sans: RequestPolicyX509PropsSans {
											dns_names: &[
												common_name,
												&format!("*.{common_name}"),
											],
										},
										subject: format_args!("CN={common_name}"),
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
				status: http_common::StatusCode,
				body: Option<&mut http_common::ResponseBody<impl std::io::Read>>,
				_headers: http_common::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				#[derive(serde::Deserialize)]
				struct ResponseInner<'a> {
					attributes: ResponseAttributes,
					#[serde(borrow)]
					id: std::borrow::Cow<'a, str>,
				}

				#[derive(serde::Deserialize)]
				struct ResponseAttributes {
					exp: i64,
				}

				Ok(match (status, body) {
					(http_common::StatusCode::OK, Some(body)) => {
						let ResponseInner { attributes: ResponseAttributes { exp }, id } = body.as_json()?;

						let not_after = time::OffsetDateTime::from_unix_timestamp(exp).context("certificate expiry out of range")?;

						let version = match id.rsplit_once('/') {
							Some((_, version)) => version.to_owned(),
							None => id.into_owned(),
						};

						Some(Response(Some(Certificate {
							version,
							not_after,
						})))
					},

					(http_common::StatusCode::NOT_FOUND, _) => Some(Response(None)),

					_ => None,
				})
			}
		}

		let certificate =
			self.logger.report_operation( "azure/key_vault/certificate", (self.key_vault_name, certificate_name), <log2::ScopedObjectOperation>::Get, async {
				let Response(certificate) =
					crate::request(
						self,
						http_common::Method::GET,
						format_args!("/certificates/{certificate_name}?api-version=7.3"),
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
				status: http_common::StatusCode,
				_body: Option<&mut http_common::ResponseBody<impl std::io::Read>>,
				_headers: http_common::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				Ok(match status {
					http_common::StatusCode::CREATED => Some(Response),
					_ => None,
				})
			}
		}

		self.logger.report_operation(
			"azure/key_vault/certificate",
			(self.key_vault_name, certificate_name),
			log2::ScopedObjectOperation::Create { value: format_args!("{certificates:?}") },
			async {
				let _: Response =
					crate::request(
						self,
						http_common::Method::POST,
						format_args!("/certificates/{certificate_name}/pending/merge?api-version=7.3"),
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
		curve: acme::EcCurve,
		exportable: bool,
	},

	EcHsm {
		curve: acme::EcCurve,
	},

	Rsa {
		num_bits: u16,
		exportable: bool,
	},

	RsaHsm {
		num_bits: u16,
	},
}

impl serde::Serialize for CreateCsrKeyType {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: serde::Serializer {
		use serde::ser::SerializeMap;

		let mut serializer = serializer.serialize_map(Some(3))?;

		match self {
			CreateCsrKeyType::Ec { curve, exportable } => {
				serializer.serialize_entry("kty", "EC")?;
				serializer.serialize_entry("crv", curve)?;
				serializer.serialize_entry("exportable", exportable)?;
			},

			CreateCsrKeyType::EcHsm { curve } => {
				serializer.serialize_entry("kty", "EC-HSM")?;
				serializer.serialize_entry("crv", curve)?;
				serializer.serialize_entry("exportable", &false)?;
			},

			CreateCsrKeyType::Rsa { num_bits, exportable } => {
				serializer.serialize_entry("kty", "RSA")?;
				serializer.serialize_entry("key_size", num_bits)?;
				serializer.serialize_entry("exportable", exportable)?;
			},

			CreateCsrKeyType::RsaHsm { num_bits } => {
				serializer.serialize_entry("kty", "RSA-HSM")?;
				serializer.serialize_entry("key_size", num_bits)?;
				serializer.serialize_entry("exportable", &false)?;
			},
		}

		serializer.end()
	}
}

#[derive(Debug)]
pub struct Certificate {
	pub version: String,
	pub not_after: time::OffsetDateTime,
}
