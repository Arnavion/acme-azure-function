module acme_azure_function.CheckKeyVaultCertificate

open Microsoft.Extensions.Logging

type Request = {
    SubscriptionID: string
    ResourceGroupName: string
    AzureAuth: Azure.Auth
    KeyVaultName: string
    KeyVaultCertificateName: string
}

type Response = {
    CertificateExpiry: System.DateTime option
}

[<Microsoft.Azure.WebJobs.FunctionName("CheckKeyVaultCertificate")>]
[<Microsoft.Azure.WebJobs.Singleton>]
let Run
    ([<Microsoft.Azure.WebJobs.ActivityTrigger>] request: Request)
    (log: Microsoft.Extensions.Logging.ILogger)
    (cancellationToken: System.Threading.CancellationToken)
    : System.Threading.Tasks.Task<Response> =
    Common.Function "BeginAcmeOrder" log (fun () -> FSharp.Control.Tasks.Builders.task {
        let azureAccount =
            new Azure.Account (
                request.SubscriptionID,
                request.ResourceGroupName,
                request.AzureAuth,
                log,
                cancellationToken
            );


        log.LogInformation (
            "Getting expiry of certificate {keyVaultName}/{keyVaultCertificateName} ...",
            request.KeyVaultName,
            request.KeyVaultCertificateName
        )

        let! certificate =
            azureAccount.GetKeyVaultCertificate
                request.KeyVaultName
                request.KeyVaultCertificateName

        let certificateExpiry =
            match certificate with
            | Some certificate ->
                log.LogInformation (
                    "Certificate {keyVaultName}/{keyVaultCertificateName} expires at {expiry}",
                    request.KeyVaultName,
                    request.KeyVaultCertificateName,
                    certificate.Expiry.ToString "o"
                )
                Some certificate.Expiry
            | None ->
                log.LogInformation (
                    "Certificate {keyVaultName}/{keyVaultCertificateName} does not exist",
                    request.KeyVaultName,
                    request.KeyVaultCertificateName
                )
                None

        return {
            CertificateExpiry = certificateExpiry
        }
    })
