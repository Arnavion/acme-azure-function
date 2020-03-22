module ArnavionDev.AzureFunctions.AcmeFunction.Acme

open Microsoft.Extensions.Logging

[<Struct; System.Runtime.Serialization.DataContract>]
type private AccountKeyParameters = {
    [<field:System.Runtime.Serialization.DataMember(Name = "D")>]
    D: byte array
    [<field:System.Runtime.Serialization.DataMember(Name = "QX")>]
    QX: byte array
    [<field:System.Runtime.Serialization.DataMember(Name = "QY")>]
    QY: byte array
}

let private NISTP384 (): System.Security.Cryptography.ECCurve =
    new System.Security.Cryptography.Oid ("1.3.132.0.34")
    |> System.Security.Cryptography.ECCurve.CreateFromOid

[<Microsoft.Azure.WebJobs.FunctionName("Acme")>]
[<Microsoft.Azure.WebJobs.Singleton>]
let rec Run
#if LOCAL
    ([<Microsoft.Azure.WebJobs.HttpTrigger("Get")>] request: obj)
#else
    ([<Microsoft.Azure.WebJobs.TimerTrigger("0 0 0 * * *")>] timerInfo: Microsoft.Azure.WebJobs.TimerInfo)
#endif
    (log: Microsoft.Extensions.Logging.ILogger)
    (cancellationToken: System.Threading.CancellationToken)
    : System.Threading.Tasks.Task =
    ArnavionDev.AzureFunctions.Common.Function "Acme" log (fun () -> FSharp.Control.Tasks.Builders.task {
#if LOCAL
        let _ = request
#else
        let _ = timerInfo
#endif

        let azureAccount =
            new ArnavionDev.AzureFunctions.RestAPI.Azure.Account (
                Settings.Instance.AzureSubscriptionID,
                Settings.Instance.AzureResourceGroupName,
                Settings.Instance.AzureAuth,
                log,
                cancellationToken
            );

        let! certificate =
            azureAccount.GetKeyVaultCertificate
                Settings.Instance.AzureKeyVaultName
                Settings.Instance.AzureKeyVaultCertificateName

        let needNewCertificate =
            certificate
            |> Option.map (fun certificate ->
                let certificateExpiry = certificate.Certificate.NotAfter.ToUniversalTime ()
                certificateExpiry < (System.DateTime.UtcNow + (System.TimeSpan.FromDays 30.0))
            )
            |> defaultArg <| true
        if needNewCertificate then
            let domainName = sprintf "*.%s" Settings.Instance.TopLevelDomainName

            let! accountKey =
                GetAcmeAccountKey
                    azureAccount
                    Settings.Instance.AzureKeyVaultName
                    Settings.Instance.AcmeAccountKeySecretName
                    log

            let! acmeAccount =
                ArnavionDev.AzureFunctions.RestAPI.Acme.GetAccount
                    Settings.Instance.AcmeDirectoryURL
                    accountKey
                    (ArnavionDev.AzureFunctions.RestAPI.Acme.AccountCreateOptions.New (ContactURL = Settings.Instance.AcmeContactURL))
                    log
                    cancellationToken

            let! acmeOrder = acmeAccount.PlaceOrder domainName

            let! orderURL = FSharp.Control.Tasks.Builders.task {
                match acmeOrder with
                | ArnavionDev.AzureFunctions.RestAPI.Acme.Order.Pending
                    (OrderURL = orderURL; AuthorizationURL = authorizationURL; ChallengeURL = challengeURL; DnsTxtRecordContent = dnsTxtRecordContent) ->
                    let! () =
                        azureAccount.SetDnsTxtRecord
                            Settings.Instance.TopLevelDomainName
                            (ArnavionDev.AzureFunctions.RestAPI.Azure.SetDnsTxtRecordAction.Create (
                                Name = "_acme-challenge",
                                Content = dnsTxtRecordContent
                            ))

                    let! acmeAuthorizationResult = FSharp.Control.Tasks.Builders.task {
                        try
                            let! () = acmeAccount.CompleteAuthorization authorizationURL challengeURL
                            return Ok ()
                        with ex ->
                            return Error ex
                    }

                    let! () =
                        azureAccount.SetDnsTxtRecord
                            Settings.Instance.TopLevelDomainName
                            (ArnavionDev.AzureFunctions.RestAPI.Azure.SetDnsTxtRecordAction.Delete (Name = "_acme-challenge"))

                    match acmeAuthorizationResult with
                    | Ok () -> return orderURL
                    | Error ex ->
                        raise ex
                        return Unchecked.defaultof<_>

                | ArnavionDev.AzureFunctions.RestAPI.Acme.Order.Ready (OrderURL = orderURL) ->
                    return orderURL
            }

            let certificatePrivateKey = System.Security.Cryptography.RSA.Create 4096

            let csr =
                log.LogInformation ("Creating CSR for {domainName} ...", domainName)

                let csr =
                    new System.Security.Cryptography.X509Certificates.CertificateRequest (
                        new System.Security.Cryptography.X509Certificates.X500DistinguishedName (sprintf "CN=%s" domainName),
                        certificatePrivateKey,
                        System.Security.Cryptography.HashAlgorithmName.SHA256,
                        System.Security.Cryptography.RSASignaturePadding.Pkcs1
                    )
                let csr = csr.CreateSigningRequest ()

                log.LogInformation ("Created CSR for {domainName}", domainName)

                csr

            let! certificateCollection = acmeAccount.FinalizeOrder orderURL csr

            log.LogInformation "Updating certificate with private key..."

            certificateCollection.[0] <-
                System.Security.Cryptography.X509Certificates.RSACertificateExtensions.CopyWithPrivateKey (
                    certificateCollection.[0],
                    certificatePrivateKey
                )

            log.LogInformation "Updated certificate with private key"

            let! () =
                azureAccount.SetKeyVaultCertificate
                    Settings.Instance.AzureKeyVaultName
                    Settings.Instance.AzureKeyVaultCertificateName
                    certificateCollection

            log.LogInformation "Certificate has been renewed"

        else
            log.LogInformation "Certificate does not need to be renewed"

        return ()
    }) :> _

and private GetAcmeAccountKey
    (azureAccount: ArnavionDev.AzureFunctions.RestAPI.Azure.Account)
    (keyVaultName: string)
    (secretName: string)
    (log: Microsoft.Extensions.Logging.ILogger)
    : System.Threading.Tasks.Task<System.Security.Cryptography.ECDsa> =
    FSharp.Control.Tasks.Builders.task {
        let! secret =
            azureAccount.GetKeyVaultSecret
                keyVaultName
                secretName

        match secret with
        | Some secret ->
            let accountKeyParameters =
                secret |>
                ArnavionDev.AzureFunctions.Common.UTF8Encoding.GetString |>
                Newtonsoft.Json.JsonConvert.DeserializeObject<AccountKeyParameters>
            let accountKeyParameters =
                new System.Security.Cryptography.ECParameters (
                    Curve = NISTP384 (),
                    D = accountKeyParameters.D,
                    Q = new System.Security.Cryptography.ECPoint (
                        X = accountKeyParameters.QX,
                        Y = accountKeyParameters.QY
                    )
                )

            return accountKeyParameters |> System.Security.Cryptography.ECDsa.Create

        | None ->
            log.LogInformation "Generating new ACME account key..."

            let accountKey = NISTP384 () |> System.Security.Cryptography.ECDsa.Create

            log.LogInformation "Generated new ACME account key"


            let accountKeyParameters = accountKey.ExportParameters true
            let accountKeyParameters = {
                AccountKeyParameters.D = accountKeyParameters.D
                AccountKeyParameters.QX = accountKeyParameters.Q.X
                AccountKeyParameters.QY = accountKeyParameters.Q.Y
            }

            let! () =
                azureAccount.SetKeyVaultSecret
                    keyVaultName
                    secretName
                    (accountKeyParameters |> Newtonsoft.Json.JsonConvert.SerializeObject |> ArnavionDev.AzureFunctions.Common.UTF8Encoding.GetBytes)


            return accountKey
    }
