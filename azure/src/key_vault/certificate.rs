impl super::Client<'_> {
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
							format_args!("/certificates/{certificate_name}/create?api-version=7.4"),
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
					#[serde(deserialize_with = "deserialize_base64")]
					cer: Vec<u8>,
					#[serde(borrow)]
					id: std::borrow::Cow<'a, str>,
				}

				fn deserialize_base64<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error> where D: serde::Deserializer<'de> {
					struct Visitor;

					impl serde::de::Visitor<'_> for Visitor {
						type Value = Vec<u8>;

						fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
							f.write_str("base64-encoded string")
						}

						fn visit_str<E>(self, s: &str) -> Result<Self::Value, E> where E: serde::de::Error {
							let value = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, s).map_err(serde::de::Error::custom)?;
							Ok(value)
						}
					}

					deserializer.deserialize_str(Visitor)
				}

				Ok(match (status, body) {
					(http_common::StatusCode::OK, Some(body)) => {
						let ResponseInner { cer, id } = body.as_json()?;

						let version = match id.rsplit_once('/') {
							Some((_, version)) => version.to_owned(),
							None => id.into_owned(),
						};

						let (trailing_garbage, cer) = x509_parser::parse_x509_certificate(&cer)?;
						if !trailing_garbage.is_empty() {
							return Err(anyhow::anyhow!("cert has trailing garbage"));
						}

						let (not_before, not_after) = {
							let validity = cer.validity();
							(validity.not_before.to_datetime(), validity.not_after.to_datetime())
						};

						let ari_id =
							cer
							.get_extension_unique(&x509_parser::oid_registry::OID_X509_EXT_AUTHORITY_KEY_IDENTIFIER)?
							.and_then(|extension| match extension.parsed_extension() {
								x509_parser::extensions::ParsedExtension::AuthorityKeyIdentifier(aki) => {
									let mut ari_id = base64::Engine::encode(&acme::JWS_BASE64_ENGINE, aki.key_identifier.as_ref()?.0);

									ari_id.push('.');

									let serial = cer.raw_serial();
									if serial.first().is_some_and(|b| b & 0x80 != 0) {
										// Non-positive serial is invalid.
										return None;
									}
									base64::Engine::encode_string(&acme::JWS_BASE64_ENGINE, serial, &mut ari_id);

									Some(ari_id)
								},
								_ => None,
							});

						Some(Response(Some(Certificate {
							version,
							ari_id,
							not_before,
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
						format_args!("/certificates/{certificate_name}?api-version=7.4"),
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
						format_args!("/certificates/{certificate_name}/pending/merge?api-version=7.4"),
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
	pub ari_id: Option<String>,
	pub not_before: time::OffsetDateTime,
	pub not_after: time::OffsetDateTime,
}
