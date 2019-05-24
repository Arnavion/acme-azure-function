module acme_azure_function.BeginAcmeOrder

open Microsoft.Extensions.Logging

type Request = {
    AccountKey: AccountKeyParameters
    DirectoryURL: string
    ContactURL: string
    DomainName: string
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
        ChallengeBlobPath: string
        ChallengeBlobContent: string
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
    ([<Microsoft.Azure.WebJobs.ActivityTrigger>] request: Request)
    (log: Microsoft.Extensions.Logging.ILogger)
    (cancellationToken: System.Threading.CancellationToken)
    : System.Threading.Tasks.Task<Response> =
    Common.Function "BeginAcmeOrder" log (fun () -> FSharp.Control.Tasks.Builders.task {
        log.LogInformation "Getting ACME account..."

        let! acmeAccount =
            Acme.GetAccount
                request.DirectoryURL
                ({
                    D = request.AccountKey.D |> System.Convert.FromBase64String
                    QX = request.AccountKey.QX |> System.Convert.FromBase64String
                    QY = request.AccountKey.QY |> System.Convert.FromBase64String
                })
                (Acme.AccountCreateOptions.New request.ContactURL)
                log
                cancellationToken

        log.LogInformation "Got ACME account"


        log.LogInformation ("Creating order for {domainName} ...", request.DomainName)

        let! order = acmeAccount.BeginOrder request.DomainName

        log.LogInformation ("Created order for {domainName} : {order}", request.DomainName, (sprintf "%O" order))


        return
            match order with
            | Acme.Order.Pending pending ->
                Response.Pending
                    {|
                        AccountURL = pending.AccountURL
                        OrderURL = pending.OrderURL
                        AuthorizationURL = pending.AuthorizationURL
                        ChallengeURL = pending.ChallengeURL
                        ChallengeBlobPath = pending.ChallengeBlobPath
                        ChallengeBlobContent = pending.ChallengeBlobContent |> System.Convert.ToBase64String
                    |}

            | Acme.Order.Ready ready ->
                Response.Ready
                    {|
                        AccountURL = ready.AccountURL
                        OrderURL = ready.OrderURL
                    |}

            | Acme.Order.Valid certificate ->
                Response.Valid (certificate |> System.Convert.ToBase64String)
    })
