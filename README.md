This repository contains two Azure Functions:

- `AcmeFunction` provisions a wildcard TLS certificate for a domain and stores it in an Azure KeyVault. The certificate is provisioned using [the ACME v2 protocol.](https://ietf-wg-acme.github.io/acme/draft-ietf-acme-acme.html)

- `UpdateCdnCertificate` monitors an Azure CDN endpoint's custom domain configured to use an HTTPS certificate from an Azure KeyVault. It ensures the CDN endpoint is using the latest certificate in the KeyVault.

Together, these functions can be used to have an Azure CDN endpoint serve HTTPS using a certificate from an ACME provider.

This setup is used for <https://www.arnavion.dev>, which is a static website in an Azure Storage Account with an Azure CDN endpoint in front. [Let's Encrypt](https://letsencrypt.org/) is used as the ACME server.

See the READMEs inside the individual directories for how to use them.


# License

```
acme-azure-function

https://github.com/Arnavion/acme-azure-function

Copyright 2019 Arnav Singh

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

   http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
```
