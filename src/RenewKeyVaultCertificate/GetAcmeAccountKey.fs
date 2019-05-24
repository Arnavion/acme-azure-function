module acme_azure_function.GetAcmeAccountKey

open Microsoft.Extensions.Logging

type Request = {
    SubscriptionID: string
    ResourceGroupName: string
    AzureAuth: Azure.Auth
    KeyVaultName: string
    AccountKeySecretName: string
}

type Response = BeginAcmeOrder.AccountKeyParameters

[<Microsoft.Azure.WebJobs.FunctionName("GetAcmeAccountKey")>]
[<Microsoft.Azure.WebJobs.Singleton>]
let Run
    ([<Microsoft.Azure.WebJobs.ActivityTrigger>] request: Request)
    (log: Microsoft.Extensions.Logging.ILogger)
    (cancellationToken: System.Threading.CancellationToken)
    : System.Threading.Tasks.Task<Response> =
    Common.Function "GetAcmeAccountKey" log (fun () -> FSharp.Control.Tasks.Builders.task {
        log.LogInformation "Getting Azure account..."

        let azureAccount =
            new Azure.Account (
                request.SubscriptionID,
                request.ResourceGroupName,
                request.AzureAuth,
                log,
                cancellationToken
            )

        log.LogInformation "Got Azure account"


        log.LogInformation (
            "Getting ACME account key secret {keyVauleName}/{keyVaultSecretName} ...",
            request.KeyVaultName,
            request.AccountKeySecretName
        )

        let! accountKey =
            azureAccount.GetKeyVaultSecret
                request.KeyVaultName
                request.AccountKeySecretName

        let! accountKey = FSharp.Control.Tasks.Builders.task {
            match accountKey with
            | Some accountKey ->
                log.LogInformation (
                    "Got ACME account key secret {keyVauleName}/{keyVaultSecretName}",
                    request.KeyVaultName,
                    request.AccountKeySecretName
                )
                return accountKey |> Common.UTF8Encoding.GetString |> Newtonsoft.Json.JsonConvert.DeserializeObject<BeginAcmeOrder.AccountKeyParameters>

            | None ->
                log.LogInformation (
                    "ACME account key secret {keyVauleName}/{keyVaultSecretName} does not exist. Generating new key...",
                    request.KeyVaultName,
                    request.AccountKeySecretName
                )

                let key = Common.NISTP384 () |> System.Security.Cryptography.ECDsa.Create
                let keyParameters = key.ExportParameters true
                let accountKey = {
                    BeginAcmeOrder.AccountKeyParameters.D = keyParameters.D |> System.Convert.ToBase64String
                    BeginAcmeOrder.AccountKeyParameters.QX = keyParameters.Q.X |> System.Convert.ToBase64String
                    BeginAcmeOrder.AccountKeyParameters.QY = keyParameters.Q.Y |> System.Convert.ToBase64String
                }

                log.LogInformation "Generated new ACME account key"


                log.LogInformation (
                    "Uploading new ACME account key to secret {keyVauleName}/{keyVaultSecretName} ...",
                    request.KeyVaultName,
                    request.AccountKeySecretName
                )

                let! () =
                    azureAccount.SetKeyVaultSecret
                        request.KeyVaultName
                        request.AccountKeySecretName
                        (accountKey |> Newtonsoft.Json.JsonConvert.SerializeObject |> Common.UTF8Encoding.GetBytes)

                log.LogInformation (
                    "Uploaded new ACME account key to secret {keyVauleName}/{keyVaultSecretName}",
                    request.KeyVaultName,
                    request.AccountKeySecretName
                )


                return accountKey
        }

        return accountKey
    })
