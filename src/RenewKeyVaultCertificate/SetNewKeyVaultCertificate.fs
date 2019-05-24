module acme_azure_function.SetNewKeyVaultCertificate

open Microsoft.Extensions.Logging

type Request = {
    SubscriptionID: string
    ResourceGroupName: string
    AzureAuth: Azure.Auth
    Certificate: string
    CertificatePrivateKey: CreateCsr.KeyParameters
    DomainName: string
    KeyVaultName: string
    KeyVaultCertificateName: string
}

type Response = unit

[<Microsoft.Azure.WebJobs.FunctionName("SetNewKeyVaultCertificate")>]
[<Microsoft.Azure.WebJobs.Singleton>]
let Run
    ([<Microsoft.Azure.WebJobs.ActivityTrigger>] request: Request)
    (log: Microsoft.Extensions.Logging.ILogger)
    (cancellationToken: System.Threading.CancellationToken)
    : System.Threading.Tasks.Task<Response> =
    Common.Function "SetNewKeyVaultCertificate" log (fun () -> FSharp.Control.Tasks.Builders.task {
        let azureAccount =
            new Azure.Account (
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

        let mutable certificateKeyParameters = new System.Security.Cryptography.RSAParameters ()
        certificateKeyParameters.D <- request.CertificatePrivateKey.D |> System.Convert.FromBase64String
        certificateKeyParameters.DP <- request.CertificatePrivateKey.DP |> System.Convert.FromBase64String
        certificateKeyParameters.DQ <- request.CertificatePrivateKey.DQ |> System.Convert.FromBase64String
        certificateKeyParameters.Exponent <- request.CertificatePrivateKey.Exponent |> System.Convert.FromBase64String
        certificateKeyParameters.InverseQ <- request.CertificatePrivateKey.InverseQ |> System.Convert.FromBase64String
        certificateKeyParameters.Modulus <- request.CertificatePrivateKey.Modulus |> System.Convert.FromBase64String
        certificateKeyParameters.P <- request.CertificatePrivateKey.P |> System.Convert.FromBase64String
        certificateKeyParameters.Q <- request.CertificatePrivateKey.Q |> System.Convert.FromBase64String
        let certificateKey = certificateKeyParameters |> System.Security.Cryptography.RSA.Create

        log.LogInformation "Imported private key"


        log.LogInformation "Updating certificate with private key..."

        let expectedSubjectDistinguishedName = (sprintf "CN=%s" request.DomainName)
        let targetCertificateCollection = new System.Security.Cryptography.X509Certificates.X509Certificate2Collection ()
        let mutable foundCertificate = false
        let enumerator = certificateCollection.GetEnumerator ()
        while enumerator.MoveNext() do
            let certificate = enumerator.Current
            let certificate =
                if certificate.SubjectName.Name = expectedSubjectDistinguishedName then
                    if foundCertificate then
                        failwith "More than one certificate has the expected subject distinguished name"
                    foundCertificate <- true
                    System.Security.Cryptography.X509Certificates.RSACertificateExtensions.CopyWithPrivateKey (
                        certificate,
                        certificateKey
                    )
                else
                    certificate
            targetCertificateCollection.Add(certificate) |> ignore
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
