module ArnavionDev.AzureFunctions.AcmeFunction.BeginAcmeOrder

open Microsoft.Extensions.Logging

type Request = {
    AccountKey: AccountKeyParameters
    DirectoryURL: string
    ContactURL: string
    TopLevelDomainName: string
}
and AccountKeyParameters = {
    D: string
    QX: string
    QY: string
}

type Response =
| Pending of
    {|
        AccountURL: string
        OrderURL: string
        AuthorizationURL: string
        ChallengeURL: string
        DnsTxtRecordContent: string
    |}
| Ready of
    {|
        AccountURL: string
        OrderURL: string
    |}
| Valid of Certificate: string

[<Microsoft.Azure.WebJobs.FunctionName("BeginAcmeOrder")>]
[<Microsoft.Azure.WebJobs.Singleton>]
let Run
    ([<Microsoft.Azure.WebJobs.Extensions.DurableTask.ActivityTrigger>] request: Request)
    (log: Microsoft.Extensions.Logging.ILogger)
    (cancellationToken: System.Threading.CancellationToken)
    : System.Threading.Tasks.Task<Response> =
    ArnavionDev.AzureFunctions.Common.Function "BeginAcmeOrder" log (fun () -> FSharp.Control.Tasks.Builders.task {
        log.LogInformation "Getting ACME account..."

        let! acmeAccount =
            ArnavionDev.AzureFunctions.RestAPI.Acme.GetAccount
                request.DirectoryURL
                ({
                    D = request.AccountKey.D |> System.Convert.FromBase64String
                    QX = request.AccountKey.QX |> System.Convert.FromBase64String
                    QY = request.AccountKey.QY |> System.Convert.FromBase64String
                })
                (ArnavionDev.AzureFunctions.RestAPI.Acme.AccountCreateOptions.New request.ContactURL)
                log
                cancellationToken

        log.LogInformation "Got ACME account"


        let domainName = sprintf "*.%s" request.TopLevelDomainName

        log.LogInformation ("Creating order for {domainName} ...", domainName)

        let! order = acmeAccount.BeginOrder domainName

        log.LogInformation ("Created order for {domainName} : {order}", domainName, (sprintf "%O" order))


        return
            match order with
            | ArnavionDev.AzureFunctions.RestAPI.Acme.Order.Pending pending ->
                Response.Pending
                    {|
                        AccountURL = pending.AccountURL
                        OrderURL = pending.OrderURL
                        AuthorizationURL = pending.AuthorizationURL
                        ChallengeURL = pending.ChallengeURL
                        DnsTxtRecordContent = pending.DnsTxtRecordContent
                    |}

            | ArnavionDev.AzureFunctions.RestAPI.Acme.Order.Ready ready ->
                Response.Ready
                    {|
                        AccountURL = ready.AccountURL
                        OrderURL = ready.OrderURL
                    |}

            | ArnavionDev.AzureFunctions.RestAPI.Acme.Order.Valid certificate ->
                Response.Valid (certificate |> System.Convert.ToBase64String)
    })
