module ArnavionDev.AzureFunctions.AcmeFunction.SetDnsTxtRecord

open Microsoft.Extensions.Logging

type Request = {
    SubscriptionID: string
    ResourceGroupName: string
    AzureAuth: ArnavionDev.AzureFunctions.RestAPI.Azure.Auth
    TopLevelDomainName: string
    Action: ArnavionDev.AzureFunctions.RestAPI.Azure.SetDnsTxtRecordAction
}

type Response = unit

[<Microsoft.Azure.WebJobs.FunctionName("SetDnsTxtRecord")>]
[<Microsoft.Azure.WebJobs.Singleton>]
let Run
    ([<Microsoft.Azure.WebJobs.Extensions.DurableTask.ActivityTrigger>] request: Request)
    (log: Microsoft.Extensions.Logging.ILogger)
    (cancellationToken: System.Threading.CancellationToken)
    : System.Threading.Tasks.Task<Response> =
    ArnavionDev.AzureFunctions.Common.Function "SetDnsTxtRecord" log (fun () -> FSharp.Control.Tasks.Builders.task {
        let azureAccount =
            new ArnavionDev.AzureFunctions.RestAPI.Azure.Account (
                request.SubscriptionID,
                request.ResourceGroupName,
                request.AzureAuth,
                log,
                cancellationToken
            );

        log.LogInformation (
            "Setting DNS TXT record in DNS zone {dnsZoneName} to {action} ...",
            request.TopLevelDomainName,
            (sprintf "%O" request.Action)
        )
        let! () = azureAccount.SetDnsTxtRecord request.TopLevelDomainName request.Action
        log.LogInformation (
            "Set challenge blob in storage account {dnsZoneName} to {action}",
            request.TopLevelDomainName,
            (sprintf "%O" request.Action)
        )

        return ()
    })
