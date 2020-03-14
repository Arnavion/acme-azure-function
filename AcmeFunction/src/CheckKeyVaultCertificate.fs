module ArnavionDev.AzureFunctions.AcmeFunction.CheckKeyVaultCertificate

open Microsoft.Extensions.Logging

type Request = {
    SubscriptionID: string
    ResourceGroupName: string
    AzureAuth: ArnavionDev.AzureFunctions.RestAPI.Azure.Auth
    KeyVaultName: string
    KeyVaultCertificateName: string
}

type Response = {
    Valid: bool
}

[<Microsoft.Azure.WebJobs.FunctionName("CheckKeyVaultCertificate")>]
[<Microsoft.Azure.WebJobs.Singleton>]
let Run
    ([<Microsoft.Azure.WebJobs.Extensions.DurableTask.ActivityTrigger>] request: Request)
    (log: Microsoft.Extensions.Logging.ILogger)
    (cancellationToken: System.Threading.CancellationToken)
    : System.Threading.Tasks.Task<Response> =
    ArnavionDev.AzureFunctions.Common.Function "CheckKeyVaultCertificate" log (fun () -> FSharp.Control.Tasks.Builders.task {
        let azureAccount =
            new ArnavionDev.AzureFunctions.RestAPI.Azure.Account (
                request.SubscriptionID,
                request.ResourceGroupName,
                request.AzureAuth,
                log,
                cancellationToken
            );


        log.LogInformation (
            "Getting secret {keyVaultName}/{keyVaultCertificateName} ...",
            request.KeyVaultName,
            request.KeyVaultCertificateName
        )

        // Get secret instead of certificate, because it has the full cert chain
        let! secret =
            azureAccount.GetKeyVaultSecret
                request.KeyVaultName
                request.KeyVaultCertificateName
        let! valid = FSharp.Control.Tasks.Builders.task {
            match secret with
            | Some secret ->
                let certificateCollection = new System.Security.Cryptography.X509Certificates.X509Certificate2Collection ()
                secret |> certificateCollection.Import

                let certificateExpiry = certificateCollection.[0].NotAfter.ToUniversalTime()
                log.LogInformation (
                    "Certificate {keyVaultName}/{keyVaultCertificateName} expires at {certificateExpiry}",
                    request.KeyVaultName,
                    request.KeyVaultCertificateName,
                    certificateExpiry.ToString "o"
                )

                if certificateExpiry < (System.DateTime.UtcNow + (System.TimeSpan.FromDays 30.0)) then
                    return false

                else
                    let certificates =
                        certificateCollection
                        |> Seq.cast<System.Security.Cryptography.X509Certificates.X509Certificate2>
                        |> List.ofSeq

                    let client = new System.Net.Http.HttpClient ()
                    let rng = System.Security.Cryptography.RandomNumberGenerator.Create ()

                    let mutable valid = true
                    let certificatesToVerify = (certificates |> Seq.pairwise).GetEnumerator ()
                    while valid && certificatesToVerify.MoveNext () do
                        let (issuer, certificate) = certificatesToVerify.Current
                        let! certIsValid = Ocsp.Verify client rng certificate issuer log cancellationToken
                        if certIsValid then
                            log.LogInformation (
                                "Certificate {certificateSubjectName} issued by {certificateIssuerName} is valid",
                                certificate.SubjectName.Name,
                                certificate.IssuerName.Name
                            )
                        else
                            log.LogInformation (
                                "Certificate {certificateSubjectName} issued by {certificateIssuerName} is not valid",
                                certificate.SubjectName.Name,
                                certificate.IssuerName.Name
                            )
                            valid <- false

                    return valid

            | None ->
                log.LogInformation (
                    "Certificate {keyVaultName}/{keyVaultCertificateName} does not exist",
                    request.KeyVaultName,
                    request.KeyVaultCertificateName
                )
                return false
        }

        return {
            Valid = valid
        }
    })
