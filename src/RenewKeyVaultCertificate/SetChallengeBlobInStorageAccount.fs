module acme_azure_function.SetChallengeBlobInStorageAccount

open Microsoft.Extensions.Logging

type Request = {
    SubscriptionID: string
    ResourceGroupName: string
    AzureAuth: Azure.Auth
    StorageAccountName: string
    Action: Action
}
and Action =
| Create of Path: string * Content: string
| Delete of Path: string

type Response = unit

[<Microsoft.Azure.WebJobs.FunctionName("SetChallengeBlobInStorageAccount")>]
[<Microsoft.Azure.WebJobs.Singleton>]
let Run
    ([<Microsoft.Azure.WebJobs.ActivityTrigger>] request: Request)
    (log: Microsoft.Extensions.Logging.ILogger)
    (cancellationToken: System.Threading.CancellationToken)
    : System.Threading.Tasks.Task<Response> =
    Common.Function "SetChallengeBlobInStorageAccount" log (fun () -> FSharp.Control.Tasks.Builders.task {
        let azureAccount =
            new Azure.Account (
                request.SubscriptionID,
                request.ResourceGroupName,
                request.AzureAuth,
                log,
                cancellationToken
            );

        let enableHttp, action =
            match request.Action with
            | Create (path, content) ->
                true, Azure.StorageAccountBlobAction.Create (
                    (sprintf "/$web%s" path),
                    content |> System.Convert.FromBase64String
                )
            | Delete path ->
                false, Azure.StorageAccountBlobAction.Delete
                    (sprintf "/$web%s" path)

        let setEnableHttpAccessOnStorageAccountResponse = FSharp.Control.Tasks.Builders.unitTask {
            log.LogInformation (
                "Setting HTTP access on storage account {storageAccountName} to {enableHttp} ...",
                request.StorageAccountName,
                enableHttp
            )
            let! () = azureAccount.SetStorageAccountEnableHttpAccess request.StorageAccountName enableHttp
            log.LogInformation (
                "Set HTTP access on storage account {storageAccountName} to {enableHttp}",
                request.StorageAccountName,
                enableHttp
            )
        }

        let setBlobInStorageAccountResponse = FSharp.Control.Tasks.Builders.unitTask {
            log.LogInformation (
                "Setting challenge blob in storage account {storageAccountName} to {action} ...",
                request.StorageAccountName,
                (sprintf "%O" action)
            )
            let! () = azureAccount.SetStorageAccountBlob request.StorageAccountName action
            log.LogInformation (
                "Set challenge blob in storage account {storageAccountName} to {action}",
                request.StorageAccountName,
                (sprintf "%O" action)
            )
        }

        let! () = setEnableHttpAccessOnStorageAccountResponse
        let! () = setBlobInStorageAccountResponse

        return ()
    })
