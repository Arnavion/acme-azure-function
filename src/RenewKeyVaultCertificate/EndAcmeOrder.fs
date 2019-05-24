module acme_azure_function.EndAcmeOrder

open Microsoft.Extensions.Logging

type Request = {
    AccountKey: BeginAcmeOrder.AccountKeyParameters
    AccountURL: string
    DirectoryURL: string
    OrderURL: string
    PendingChallenge: Acme.EndOrderChallengeParameters option
    Csr: string
}

type Response = {
    Certificate: string
}

[<Microsoft.Azure.WebJobs.FunctionName("EndAcmeOrder")>]
[<Microsoft.Azure.WebJobs.Singleton>]
let Run
    ([<Microsoft.Azure.WebJobs.ActivityTrigger>] request: Request)
    (log: Microsoft.Extensions.Logging.ILogger)
    (cancellationToken: System.Threading.CancellationToken)
    : System.Threading.Tasks.Task<Response> =
    Common.Function "EndAcmeOrder" log (fun () -> FSharp.Control.Tasks.Builders.task {
        log.LogInformation "Getting ACME account..."

        let! acmeAccount =
            Acme.GetAccount
                request.DirectoryURL
                ({
                    D = request.AccountKey.D |> System.Convert.FromBase64String
                    QX = request.AccountKey.QX |> System.Convert.FromBase64String
                    QY = request.AccountKey.QY |> System.Convert.FromBase64String
                })
                (Acme.AccountCreateOptions.Existing request.AccountURL)
                log
                cancellationToken

        log.LogInformation "Got ACME account"


        log.LogInformation "Completing order..."

        let! certificate =
            acmeAccount.EndOrder
                request.OrderURL
                request.PendingChallenge
                (request.Csr |> System.Convert.FromBase64String)

        log.LogInformation "Completed order"


        return {
            Certificate = certificate |> System.Convert.ToBase64String
        }
    })
