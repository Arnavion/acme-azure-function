This repository contains two Azure Functions:

- `acme` provisions a wildcard TLS certificate for a domain and stores it in an Azure KeyVault. The certificate is provisioned using the ACME v2 protocol.

- `update-cdn-cert` monitors an Azure CDN endpoint's custom domain configured to use an HTTPS certificate from an Azure KeyVault. It ensures the CDN endpoint is using the latest certificate in the KeyVault.

Together, these Functions can be used to have an Azure CDN endpoint serve HTTPS using a certificate from an ACME provider. Or they can be used individually, to only request new certs via ACME v2 into a KeyVault, or only keep a CDN endpoint's cert in sync with the KeyVault.

This setup is used for <https://www.arnavion.dev>, which is a static website in an Azure Storage Account with an Azure CDN endpoint in front. [Let's Encrypt](https://letsencrypt.org/) is used as the ACME server.

See the READMEs inside the individual directories for how to use them.


# Old F# versions

For the old F# versions of these Functions, see [the `fsharp` branch.](https://github.com/Arnavion/acme-azure-function/tree/fsharp) These versions are no longer maintained.

The Rust versions have a few differences compared to the F# versions:

- The F# versions had a bunch of dependency hell from Microsoft .Net libraries, like pulling multiple versions of `Newtonsoft.Json`

- The F# versions use standard structural logging available to .Net functions via the `Microsoft.Extensions.Logging` library. These logs were reported to App Insights / Log Analytics via the Functions host. The Rust versions log directly to Log Analytics instead, using its Data Collector API. This means they require the Log Analytics Workspace's shared key in the secret settings.

- The F# version of the `acme` Function worked with the ACME account key and cert private key in memory, and imported/exported them to/from KeyVault for this. The Rust version lets the KeyVault create the keys and uses KeyVault API to sign them, without ever needing them to be exported to the Function.

- The F# version of the `acme` function was limited to running on a Linux Consumption plan, due to .Net on Windows marking certificate private keys as non-exportable and preventing the cert from being used with Azure CDN. The Rust version does its crypto in pure Rust, so it does not have this limitation. However the build scripts in this repository still only support building a Linux binary, and now also require a Linux build host. (Choice of distro does not matter; the binary is built in an alpine container and is statically linked, so will run on any Linux machine including the Function App). If you want to build a Windows binary on a Windows build host, you'll need to adapt the scripts to your needs.


# License

```
acme-azure-function

https://github.com/Arnavion/acme-azure-function

Copyright 2021 Arnav Singh

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
