module acme_azure_function.UpdateCdnCertificate

open Microsoft.Extensions.Logging

[<Microsoft.Azure.WebJobs.FunctionName("UpdateCdnCertificate")>]
[<Microsoft.Azure.WebJobs.Singleton>]
let Run
    ([<Microsoft.Azure.WebJobs.TimerTrigger("0 0 0 * * *")>] timerInfo: Microsoft.Azure.WebJobs.TimerInfo)
    // ([<Microsoft.Azure.WebJobs.HttpTrigger("Get")>] request: obj)
    (log: Microsoft.Extensions.Logging.ILogger)
    (cancellationToken: System.Threading.CancellationToken)
    : System.Threading.Tasks.Task =
    Common.Function "UpdateCdnCertificate" log (fun () -> FSharp.Control.Tasks.Builders.task {
        let _ = timerInfo
        // let _ = request

        let azureAccount =
            new Azure.Account (
                Settings.Instance.AzureSubscriptionID,
                Settings.Instance.AzureResourceGroupName,
                Settings.Instance.AzureAuth,
                log,
                cancellationToken
            )


        let latestCertificateVersion = FSharp.Control.Tasks.Builders.task {
            log.LogInformation (
                "Getting latest version of certificate {keyVaultName}/{keyVaultCertificateName} ...",
                Settings.Instance.AzureKeyVaultName,
                Settings.Instance.AzureKeyVaultCertificateName
            )

            let! latestCertificate =
                azureAccount.GetKeyVaultCertificate
                    Settings.Instance.AzureKeyVaultName
                    Settings.Instance.AzureKeyVaultCertificateName
            let latestCertificateVersion = latestCertificate |> Option.map (fun latestCertificate -> latestCertificate.Version)

            match latestCertificateVersion with
            | Some latestCertificateVersion ->
                log.LogInformation (
                    "Latest version of certificate {keyVaultName}/{keyVaultCertificateName} is {certificateVersion}",
                    Settings.Instance.AzureKeyVaultName,
                    Settings.Instance.AzureKeyVaultCertificateName,
                    latestCertificateVersion
                )
            | None ->
                log.LogInformation (
                    "Certificate {keyVaultName}/{keyVaultCertificateName} does not exist",
                    Settings.Instance.AzureKeyVaultName,
                    Settings.Instance.AzureKeyVaultCertificateName
                )

            return latestCertificateVersion
        }


        let cdnCustomDomainSecretVersion = FSharp.Control.Tasks.Builders.task {
            log.LogInformation (
                "Getting version of current certificate of CDN custom domain {cdnProfileName}/{cdnEndpointName}/{cdnCustomDomainName}",
                Settings.Instance.AzureCdnProfileName,
                Settings.Instance.AzureCdnEndpointName,
                Settings.Instance.AzureCdnCustomDomainName
            )

            let! cdnCustomDomainSecretVersion =
                azureAccount.GetCdnCustomDomainCertificate
                    Settings.Instance.AzureCdnProfileName
                    Settings.Instance.AzureCdnEndpointName
                    Settings.Instance.AzureCdnCustomDomainName

            match cdnCustomDomainSecretVersion with
            | Some cdnCustomDomainSecretVersion ->
                log.LogInformation (
                    "Current certificate of CDN custom domain {cdnProfileName}/{cdnEndpointName}/{cdnCustomDomainName} has version {certificateVersion}",
                    Settings.Instance.AzureCdnProfileName,
                    Settings.Instance.AzureCdnEndpointName,
                    Settings.Instance.AzureCdnCustomDomainName,
                    cdnCustomDomainSecretVersion
                )
            | None ->
                log.LogInformation (
                    "CDN custom domain {cdnProfileName}/{cdnEndpointName}/{cdnCustomDomainName} does not have a certificate",
                    Settings.Instance.AzureCdnProfileName,
                    Settings.Instance.AzureCdnEndpointName,
                    Settings.Instance.AzureCdnCustomDomainName
                )
                log.LogInformation "CDN custom domain does not have a certificate"

            return cdnCustomDomainSecretVersion
        }


        let! latestCertificateVersion = FSharp.Control.Tasks.Builders.task {
            match! latestCertificateVersion with
            | Some latestCertificateVersion ->
                match! cdnCustomDomainSecretVersion with
                | Some cdnCustomDomainSecretVersion when cdnCustomDomainSecretVersion = latestCertificateVersion ->
                    return None
                | _ ->
                    return Some latestCertificateVersion
            | None ->
                return None
        }

        let! () = FSharp.Control.Tasks.Builders.unitTask {
            match latestCertificateVersion with
            | Some latestCertificateVersion ->
                log.LogInformation (
                    "Updating certificate for CDN custom domain {cdnProfileName}/{cdnEndpointName}/{cdnCustomDomainName} to {certificateVersion} ...",
                    Settings.Instance.AzureCdnProfileName,
                    Settings.Instance.AzureCdnEndpointName,
                    Settings.Instance.AzureCdnCustomDomainName,
                    latestCertificateVersion
                )

                let! () =
                    azureAccount.SetCdnCustomDomainCertificate
                        Settings.Instance.AzureCdnProfileName
                        Settings.Instance.AzureCdnEndpointName
                        Settings.Instance.AzureCdnCustomDomainName
                        Settings.Instance.AzureKeyVaultName
                        Settings.Instance.AzureKeyVaultCertificateName
                        latestCertificateVersion

                log.LogInformation (
                    "Updated certificate for CDN custom domain {cdnProfileName}/{cdnEndpointName}/{cdnCustomDomainName} to {certificateVersion}",
                    Settings.Instance.AzureCdnProfileName,
                    Settings.Instance.AzureCdnEndpointName,
                    Settings.Instance.AzureCdnCustomDomainName,
                    latestCertificateVersion
                )
            | None ->
                log.LogInformation (
                    "Not updating certificate for CDN custom domain {cdnProfileName}/{cdnEndpointName}/{cdnCustomDomainName}",
                    Settings.Instance.AzureCdnProfileName,
                    Settings.Instance.AzureCdnEndpointName,
                    Settings.Instance.AzureCdnCustomDomainName
                )
        }

        return ()
    }) :> _
