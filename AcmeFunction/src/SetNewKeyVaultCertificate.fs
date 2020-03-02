module ArnavionDev.AzureFunctions.AcmeFunction.SetNewKeyVaultCertificate

open Microsoft.Extensions.Logging

type Request = {
    SubscriptionID: string
    ResourceGroupName: string
    AzureAuth: ArnavionDev.AzureFunctions.RestAPI.Azure.Auth
    Certificate: string
    CertificatePrivateKey: CreateCsr.KeyParameters
    TopLevelDomainName: string
    KeyVaultName: string
    KeyVaultCertificateName: string
}

type Response = unit

[<Microsoft.Azure.WebJobs.FunctionName("SetNewKeyVaultCertificate")>]
[<Microsoft.Azure.WebJobs.Singleton>]
let Run
    ([<Microsoft.Azure.WebJobs.Extensions.DurableTask.ActivityTrigger>] request: Request)
    (log: Microsoft.Extensions.Logging.ILogger)
    (cancellationToken: System.Threading.CancellationToken)
    : System.Threading.Tasks.Task<Response> =
    ArnavionDev.AzureFunctions.Common.Function "SetNewKeyVaultCertificate" log (fun () -> FSharp.Control.Tasks.Builders.task {
        let azureAccount =
            new ArnavionDev.AzureFunctions.RestAPI.Azure.Account (
                request.SubscriptionID,
                request.ResourceGroupName,
                request.AzureAuth,
                log,
                cancellationToken
            );


        log.LogInformation "Importing certificates into certificate collection..."

        let certificateCollection = new System.Security.Cryptography.X509Certificates.X509Certificate2Collection ()
        request.Certificate |> System.Convert.FromBase64String |> certificateCollection.Import

        log.LogInformation ("Imported {numCertificates} certificates into certificate collection", certificateCollection.Count)


        log.LogInformation "Importing private key..."

        let certificateKeyParameters =
            new System.Security.Cryptography.RSAParameters (
                D = (request.CertificatePrivateKey.D |> System.Convert.FromBase64String),
                DP = (request.CertificatePrivateKey.DP |> System.Convert.FromBase64String),
                DQ = (request.CertificatePrivateKey.DQ |> System.Convert.FromBase64String),
                Exponent = (request.CertificatePrivateKey.Exponent |> System.Convert.FromBase64String),
                InverseQ = (request.CertificatePrivateKey.InverseQ |> System.Convert.FromBase64String),
                Modulus = (request.CertificatePrivateKey.Modulus |> System.Convert.FromBase64String),
                P = (request.CertificatePrivateKey.P |> System.Convert.FromBase64String),
                Q = (request.CertificatePrivateKey.Q |> System.Convert.FromBase64String)
            )
        let certificateKey = certificateKeyParameters |> System.Security.Cryptography.RSA.Create

        log.LogInformation "Imported private key"


        log.LogInformation "Updating certificate with private key..."

        let expectedSubjectDistinguishedName = (sprintf "CN=*.%s" request.TopLevelDomainName)
        let targetCertificateCollection = new System.Security.Cryptography.X509Certificates.X509Certificate2Collection ()
        let certificates =
            certificateCollection
            |> Seq.cast<System.Security.Cryptography.X509Certificates.X509Certificate2>
        let foundCertificate =
            (false, certificates)
            ||> Seq.fold (fun foundCertificate certificate ->
                let foundCertificate, certificate =
                    if certificate.SubjectName.Name = expectedSubjectDistinguishedName then
                        if foundCertificate then
                            failwith "More than one certificate has the expected subject distinguished name"
                        let certificate =
                            System.Security.Cryptography.X509Certificates.RSACertificateExtensions.CopyWithPrivateKey (
                                certificate,
                                certificateKey
                            )
                        true, certificate
                    else
                        foundCertificate, certificate
                certificate |> targetCertificateCollection.Add |> ignore
                foundCertificate
            )
        if not foundCertificate then
            failwith "No certificate has the expected subject distinguished name"

        log.LogInformation "Updated certificate with private key"


        log.LogInformation "Exporting certificate collection..."

        let certificateBytes = targetCertificateCollection.Export System.Security.Cryptography.X509Certificates.X509ContentType.Pkcs12

        log.LogInformation ("Exported certificate collection: {certificateBytes}", certificateBytes |> System.Convert.ToBase64String)


        log.LogInformation (
            "Uploading certificate to {keyVaultName}/{keyVaultCertificateName} ...",
            request.KeyVaultName,
            request.KeyVaultCertificateName
        )

        let! () =
            azureAccount.SetKeyVaultCertificate
                request.KeyVaultName
                request.KeyVaultCertificateName
                certificateBytes

        log.LogInformation (
            "Uploaded certificate to {keyVaultName}/{keyVaultCertificateName}",
            request.KeyVaultName,
            request.KeyVaultCertificateName
        )

        return ()
    })
