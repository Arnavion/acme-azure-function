This repository contains an Azure Function `function-renew-cert` that monitors a certificate in an Azure KeyVault and renews it before it expires using the ACME v2 protocol.

This function is used for the HTTPS certificate of <https://www.arnavion.dev>, which is served by an Azure CDN endpoint. [Let's Encrypt](https://letsencrypt.org/) is used as the ACME server.

See the README inside the `function-renew-cert` directory for how to use the function.


# Old F# version

For the old F# version of this Function, see [the `fsharp` branch.](https://github.com/Arnavion/acme-azure-function/tree/fsharp) That version is no longer maintained.

The Rust version has a few differences compared to the F# version:

- The F# version had a bunch of dependency hell from Microsoft .Net libraries, like pulling multiple versions of `Newtonsoft.Json`

- The F# version used standard structural logging available to .Net functions via the `Microsoft.Extensions.Logging` library. These logs were reported to App Insights / Log Analytics via the Functions host. The Rust version logs directly to Log Analytics instead, using its Data Collector API.

- The F# version worked with the ACME account key and cert private key in memory, and imported/exported them to/from KeyVault for this. The Rust version lets the KeyVault create the keys and uses KeyVault API to sign them.

- The F# version was limited to running on a Linux Consumption plan, due to .Net on Windows marking certificate private keys as non-exportable and preventing the cert from being used with Azure CDN. The Rust version does its crypto in pure Rust, so it does not have this limitation.

  However the build script in this repository still only builds a Linux binary. If you want to build a Windows binary, you'll need to adapt the script to your needs.


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
