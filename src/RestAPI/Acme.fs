module acme_azure_function.Acme

open Microsoft.Extensions.Logging

let private ConvertBytesToBase64UrlString (bytes: byte array) : string =
    let s = bytes |> System.Convert.ToBase64String
    s.Replace('+', '-').Replace('/', '_').TrimEnd('=');

let private ConvertStringToBase64UrlString (str: string) : string =
    str |> Common.UTF8Encoding.GetBytes |> ConvertBytesToBase64UrlString

[<Struct; System.Runtime.Serialization.DataContract>]
type private AcmeRequest = {
    [<field:System.Runtime.Serialization.DataMember(Name = "payload")>]
    Payload: string
    [<field:System.Runtime.Serialization.DataMember(Name = "protected")>]
    Protected: string
    [<field:System.Runtime.Serialization.DataMember(Name = "signature")>]
    Signature: string
}
and [<Struct; System.Runtime.Serialization.DataContract>] private Protected = {
    [<field:System.Runtime.Serialization.DataMember(Name = "alg")>]
    Algorithm: string
    [<field:System.Runtime.Serialization.DataMember(Name = "jwk", EmitDefaultValue = false)>]
    WebKey: System.Nullable<ECWebKey>
    [<field:System.Runtime.Serialization.DataMember(Name = "kid", EmitDefaultValue = false)>]
    KeyID: string
    [<field:System.Runtime.Serialization.DataMember(Name = "nonce")>]
    Nonce: string
    [<field:System.Runtime.Serialization.DataMember(Name = "url")>]
    URL: string
}
and [<Struct; System.Runtime.Serialization.DataContract>] internal ECWebKey = {
    [<field:System.Runtime.Serialization.DataMember(Name = "crv")>]
    Curve: string
    [<field:System.Runtime.Serialization.DataMember(Name = "kty")>]
    KeyType: string
    [<field:System.Runtime.Serialization.DataMember(Name = "x")>]
    X: string
    [<field:System.Runtime.Serialization.DataMember(Name = "y")>]
    Y: string
}
and [<Struct; System.Runtime.Serialization.DataContract>] private DirectoryResponse = {
    [<field:System.Runtime.Serialization.DataMember(Name = "newAccount")>]
    NewAccountURL: string
    [<field:System.Runtime.Serialization.DataMember(Name = "newNonce")>]
    NewNonceURL: string
    [<field:System.Runtime.Serialization.DataMember(Name = "newOrder")>]
    NewOrderURL: string
}
and [<Struct; System.Runtime.Serialization.DataContract>] private NewAccountRequest = {
    [<field:System.Runtime.Serialization.DataMember(Name = "contact")>]
    ContactURLs: string array
    [<field:System.Runtime.Serialization.DataMember(Name = "termsOfServiceAgreed")>]
    TermsOfServiceAgreed: bool
}
and [<Struct; System.Runtime.Serialization.DataContract>] private NewAccountResponse = {
    [<field:System.Runtime.Serialization.DataMember(Name = "status")>]
    Status: string
}
and [<Struct; System.Runtime.Serialization.DataContract>] private NewOrderRequest = {
    [<field:System.Runtime.Serialization.DataMember(Name = "identifiers")>]
    Identifiers: NewOrderRequest_Identifier array
}
and [<Struct; System.Runtime.Serialization.DataContract>] private NewOrderRequest_Identifier = {
    [<field:System.Runtime.Serialization.DataMember(Name = "type")>]
    Type: string
    [<field:System.Runtime.Serialization.DataMember(Name = "value")>]
    Value: string
}
and [<Struct; System.Runtime.Serialization.DataContract>] private OrderResponse = {
    [<field:System.Runtime.Serialization.DataMember(Name = "authorizations")>]
    AuthorizationURLs: string array
    [<field:System.Runtime.Serialization.DataMember(Name = "certificate")>]
    CertificateURL: string
    [<field:System.Runtime.Serialization.DataMember(Name = "finalize")>]
    FinalizeURL: string
    [<field:System.Runtime.Serialization.DataMember(Name = "status")>]
    Status: string
}
and [<Struct; System.Runtime.Serialization.DataContract>] private AuthorizationResponse = {
    [<field:System.Runtime.Serialization.DataMember(Name = "challenges")>]
    Challenges: ChallengeResponse array
    [<field:System.Runtime.Serialization.DataMember(Name = "status")>]
    Status: string
}
and [<Struct; System.Runtime.Serialization.DataContract>] private ChallengeResponse = {
    [<field:System.Runtime.Serialization.DataMember(Name = "status")>]
    Status: string
    [<field:System.Runtime.Serialization.DataMember(Name = "token")>]
    Token: string
    [<field:System.Runtime.Serialization.DataMember(Name = "type")>]
    Type: string
    [<field:System.Runtime.Serialization.DataMember(Name = "url")>]
    URL: string
}
and [<Struct; System.Runtime.Serialization.DataContract>] private ChallengeCompleteRequest = struct end
and [<Struct; System.Runtime.Serialization.DataContract>] private FinalizeOrderRequest = {
    [<field:System.Runtime.Serialization.DataMember(Name = "csr")>]
    Csr: string
}
and private AcmeAuth =
| AccountURL of string
| WebKey of ECWebKey

let inline private Request< ^a>
    (client: System.Net.Http.HttpClient)
    (log: Microsoft.Extensions.Logging.ILogger)
    (method: System.Net.Http.HttpMethod)
    (url: string)
    (payload:
        {|
            Auth: AcmeAuth
            Body: obj option
            Key: System.Security.Cryptography.ECDsa
            Nonce: string
        |} option
    )
    (expectedStatusCodes: System.Net.HttpStatusCode array)
    (serializer: Newtonsoft.Json.JsonSerializer)
    (cancellationToken: System.Threading.CancellationToken)
    : System.Threading.Tasks.Task<{| Location: string option; Nonce: string option; Response: ^a; StatusCode: System.Net.HttpStatusCode |}> =
    FSharp.Control.Tasks.Builders.task {
        let request = new System.Net.Http.HttpRequestMessage (method, url)

        let payloadParameters =
            payload |> Option.map (fun payload ->
                (
                    payload.Auth,
                    payload.Key,
                    payload.Nonce,
                    match payload.Body with
                    | Some body -> body |> Newtonsoft.Json.JsonConvert.SerializeObject |> ConvertStringToBase64UrlString
                    | None -> ""
                )
            )

        payloadParameters |> Option.iter (fun (auth, key, nonce, payloadEncoded) ->
            let keyID, webKey =
                match auth with
                | AccountURL accountURL -> Some accountURL, None
                | WebKey webKey -> None, Some webKey
            let ``protected`` = {
                Protected.Algorithm = "ES384"
                Protected.KeyID = keyID |> Option.toObj
                Protected.Nonce = nonce
                Protected.URL = url
                Protected.WebKey = webKey |> Option.toNullable
            }

            let protectedEncoded = ``protected`` |> Newtonsoft.Json.JsonConvert.SerializeObject |> ConvertStringToBase64UrlString

            let signatureInput = Array.zeroCreate (protectedEncoded.Length + 1 + payloadEncoded.Length)
            System.Buffer.BlockCopy ((protectedEncoded |> System.Text.Encoding.ASCII.GetBytes), 0, signatureInput, 0, protectedEncoded.Length)
            signatureInput.[protectedEncoded.Length] <- byte '.'
            System.Buffer.BlockCopy ((payloadEncoded |> System.Text.Encoding.ASCII.GetBytes), 0, signatureInput, protectedEncoded.Length + 1, payloadEncoded.Length)
            let signature = (key.SignData (signatureInput, System.Security.Cryptography.HashAlgorithmName.SHA384)) |> ConvertBytesToBase64UrlString

            Common.Serialize
                request
                serializer
                {
                    AcmeRequest.Payload = payloadEncoded
                    AcmeRequest.Protected = protectedEncoded
                    AcmeRequest.Signature = signature
                }
                Common.ApplicationJoseJsonContentType
        )

        let! response =
            Common.SendRequest
                client
                request
                expectedStatusCodes
                log
                cancellationToken

        let haveNewNonce, newNonce = response.Headers.TryGetValues "Replay-Nonce"
        let newNonce =
            if haveNewNonce then
                newNonce |> Seq.head |> Some
            else
                None

        let location = response.Headers.Location |> Option.ofObj |> Option.map (fun location -> location.ToString())

        let! response =
            Common.Deserialize
                serializer
                response
                (expectedStatusCodes |> Seq.map (fun statusCode -> (statusCode, typedefof< ^a>)))
                log
                cancellationToken

        let statusCode, response =
            match response with
            | statusCode, (:? ^a as response) -> statusCode, response
            | _ -> failwith "unreachable"

        return {| Location = location; Nonce = newNonce; Response = response; StatusCode = statusCode |}
    }

type internal Account internal
    (
        client: System.Net.Http.HttpClient,
        serializer: Newtonsoft.Json.JsonSerializer,
        log: Microsoft.Extensions.Logging.ILogger,
        cancellationToken: System.Threading.CancellationToken,

        accountURL: string,
        key: System.Security.Cryptography.ECDsa,
        keyJwkThumbprint: string,

        nonce: string,

        newOrderURL: string
    ) =
    let mutable nonce = nonce

    member internal __.Client = client
    member internal __.Serializer = serializer
    member internal __.Log = log
    member internal __.CancellationToken = cancellationToken

    member internal __.AccountURL = accountURL
    member internal __.Key = key
    member internal __.Nonce
        with get () = nonce
        and set (value) = nonce <- value

    member private __.KeyJwkThumbprint = keyJwkThumbprint
    member private __.NewOrderURL = newOrderURL

type internal AccountKeyParameters = {
    D: byte array
    QX: byte array
    QY: byte array
}

type internal AccountCreateOptions =
| Existing of AccountURL: string
| New of ContactURL: string

let internal GetAccount
    (directoryURL: string)
    (keyParameters: AccountKeyParameters)
    (createOptions: AccountCreateOptions)
    (log: Microsoft.Extensions.Logging.ILogger)
    (cancellationToken: System.Threading.CancellationToken)
    : System.Threading.Tasks.Task<Account> =
    FSharp.Control.Tasks.Builders.task {
        let client: System.Net.Http.HttpClient = new System.Net.Http.HttpClient()
        let serializer: Newtonsoft.Json.JsonSerializer = new Newtonsoft.Json.JsonSerializer()

        log.LogInformation ("Getting directory {directoryURL} ...", directoryURL)

        let! response =
            Request<DirectoryResponse>
                client
                log
                System.Net.Http.HttpMethod.Get
                directoryURL
                None
                [| System.Net.HttpStatusCode.OK |]
                serializer
                cancellationToken
        let nonce = response.Nonce

        let directoryResponse = response.Response
        let newAccountURL = directoryResponse.NewAccountURL
        let newNonceURL = directoryResponse.NewNonceURL
        let newOrderURL = directoryResponse.NewOrderURL

        log.LogInformation ("Got directory {directoryURL}", directoryURL)


        log.LogInformation "Importing account key..."

        let parsedKeyParameters =
            new System.Security.Cryptography.ECParameters (
                Curve = Common.NISTP384 (),
                D = keyParameters.D,
                Q = new System.Security.Cryptography.ECPoint (
                    X = keyParameters.QX,
                    Y = keyParameters.QY
                )
            )

        let key = parsedKeyParameters |> System.Security.Cryptography.ECDsa.Create

        let keyJwk = {
            ECWebKey.Curve = "P-384"
            ECWebKey.KeyType = "EC"
            ECWebKey.X = parsedKeyParameters.Q.X |> ConvertBytesToBase64UrlString
            ECWebKey.Y = parsedKeyParameters.Q.Y |> ConvertBytesToBase64UrlString
        }

        log.LogInformation "Imported account key"


        log.LogInformation "Creating account key thumbprint..."

        let sha256Hash = System.Security.Cryptography.SHA256.Create ()
        let keyJwkJSON = Newtonsoft.Json.JsonConvert.SerializeObject (keyJwk, Newtonsoft.Json.Formatting.None)
        let keyJwkThumbprint = keyJwkJSON |> Common.UTF8Encoding.GetBytes |> sha256Hash.ComputeHash |> ConvertBytesToBase64UrlString

        log.LogInformation "Created account key thumbprint"


        let! nonce = FSharp.Control.Tasks.Builders.task {
            match nonce with
            | Some nonce ->
                log.LogInformation "Already have initial nonce"
                return nonce
            | None ->
                log.LogInformation "Getting initial nonce..."
                let! response =
                    Request<Common.Empty>
                        client
                        log
                        System.Net.Http.HttpMethod.Head
                        newNonceURL
                        None
                        [| System.Net.HttpStatusCode.OK |]
                        serializer
                        cancellationToken
                match response.Nonce with
                | Some nonce ->
                    log.LogInformation "Got initial nonce"
                    return nonce
                | None ->
                    failwith "Server did not return initial nonce from NewNonce endpoint"
                    return Unchecked.defaultof<_>
        }

        let! accountURL, nonce = FSharp.Control.Tasks.Builders.task {
            match createOptions with
            | Existing accountURL ->
                return accountURL, nonce

            | New contactURL ->
                log.LogInformation "Creating / getting account corresponding to account key..."

                let! newAccountResponse =
                    Request<NewAccountResponse>
                        client
                        log
                        System.Net.Http.HttpMethod.Post
                        newAccountURL
                        (Some
                            {|
                                Auth = WebKey keyJwk
                                Body = Some ({
                                    NewAccountRequest.ContactURLs = [| contactURL |]
                                    NewAccountRequest.TermsOfServiceAgreed = true
                                } :> obj)
                                Key = key
                                Nonce = nonce
                            |}
                        )
                        [| System.Net.HttpStatusCode.Created; System.Net.HttpStatusCode.OK |]
                        serializer
                        cancellationToken
                let accountResponse = newAccountResponse.Response
                let nonce =
                    match newAccountResponse.Nonce with
                    | Some nonce -> nonce
                    | None -> failwith "Server did not return new nonce from NewAccount endpoint"

                let accountURL =
                    match newAccountResponse.Location with
                    | Some accountURL -> accountURL
                    | None -> failwith "Server did not return account URL from NewAccount endpoint"

                log.LogInformation ("Created / got account {accountURL}", accountURL)
                log.LogInformation (
                    "{acmeObject} {acmeObjectURL} has {acmeObjectStatus} status",
                    "Account",
                    accountURL,
                    accountResponse.Status
                )

                if accountResponse.Status <> "valid" then
                    failwith (sprintf "Account has %s status" accountResponse.Status)

                return accountURL, nonce
        }

        return
            new Account (
                client,
                serializer,
                log,
                cancellationToken,

                accountURL,
                key,
                keyJwkThumbprint,

                nonce,

                newOrderURL
            )
    }

let inline private AccountRequest< ^a>
    (account: Account)
    (method: System.Net.Http.HttpMethod)
    (url: string)
    (body: obj option)
    (expectedStatusCodes: System.Net.HttpStatusCode array)
    : System.Threading.Tasks.Task<(^a * string option)> =
    FSharp.Control.Tasks.Builders.task {
        let! response =
            Request< ^a>
                account.Client
                account.Log
                method
                url
                (Some (
                    {|
                        Auth = AccountURL account.AccountURL
                        Body = body
                        Key = account.Key
                        Nonce = account.Nonce
                    |}
                ))
                expectedStatusCodes
                account.Serializer
                account.CancellationToken
        account.Nonce <-
            match response.Nonce with
            | Some nonce -> nonce
            | None -> failwith "Server did not return new nonce"

        return response.Response, response.Location
    }

type Order =
| Pending of
    {|
        AccountURL: string
        OrderURL: string
        AuthorizationURL: string
        ChallengeURL: string
        ChallengeBlobPath: string
        ChallengeBlobContent: byte array
    |}
| Ready of
    {|
        AccountURL: string
        OrderURL: string
    |}
| Valid of Certificate: byte array

type EndOrderChallengeParameters = {
    AuthorizationURL: string
    ChallengeURL: string
}

type Account with
    member internal this.BeginOrder
        (domainName: string)
        : System.Threading.Tasks.Task<Order> =
        FSharp.Control.Tasks.Builders.task {
            this.Log.LogInformation ("Creating order for {domainName} ...", domainName)

            let! order, orderURL =
                AccountRequest<OrderResponse>
                    this
                    System.Net.Http.HttpMethod.Post
                    this.NewOrderURL
                    (Some ({
                        NewOrderRequest.Identifiers = [|
                            {
                                NewOrderRequest_Identifier.Type = "dns"
                                NewOrderRequest_Identifier.Value = domainName
                            }
                        |]
                    } :> obj))
                    [| System.Net.HttpStatusCode.Created |]

            let orderURL =
                match orderURL with
                | Some orderURL -> orderURL
                | None -> failwith "NewOrder endpoint did not return order URL in Location header"

            this.Log.LogInformation ("Created order for {domainName} : {orderURL}", domainName, orderURL)


            let rec DriveOrder
                (order: OrderResponse)
                : System.Threading.Tasks.Task<Order> =
                FSharp.Control.Tasks.Builders.task {
                    this.Log.LogInformation (
                        "{acmeObject} {acmeObjectURL} has {acmeObjectStatus} status",
                        "Order",
                        orderURL,
                        order.Status
                    )

                    match order.Status with
                    | "pending" ->
                        let authorizationURL =
                            match order.AuthorizationURLs with
                            | [| authorizationURL |] -> authorizationURL
                            | _ -> failwith (sprintf "Expected 1 authorization but got %d" order.AuthorizationURLs.Length)

                        let! authorization, _ =
                            AccountRequest<AuthorizationResponse>
                                this
                                System.Net.Http.HttpMethod.Post
                                authorizationURL
                                None
                                [| System.Net.HttpStatusCode.OK |]

                        this.Log.LogInformation (
                            "{acmeObject} {acmeObjectURL} has {acmeObjectStatus} status",
                            "Authorization",
                            authorizationURL,
                            authorization.Status
                        )
                        if authorization.Status <> "pending" then
                            failwith (sprintf "Authorization has %s status" authorization.Status)

                        let http01Challenge =
                            authorization.Challenges
                            |> Seq.filter (fun challenge -> challenge.Type = "http-01" && challenge.Status = "pending")
                            |> Seq.tryHead
                        let http01Challenge =
                            match http01Challenge with
                            | Some http01Challenge -> http01Challenge
                            | None -> failwith "Did not find any http-01 challenges"

                        return
                            Pending
                                {|
                                    AccountURL = this.AccountURL
                                    OrderURL = orderURL
                                    AuthorizationURL = authorizationURL
                                    ChallengeURL = http01Challenge.URL
                                    ChallengeBlobPath = (sprintf "/.well-known/acme-challenge/%s" http01Challenge.Token)
                                    ChallengeBlobContent = (sprintf "%s.%s" http01Challenge.Token this.KeyJwkThumbprint) |> Common.UTF8Encoding.GetBytes
                                |}

                    | "processing" ->
                        let! () = 1.0 |> System.TimeSpan.FromSeconds |> System.Threading.Tasks.Task.Delay

                        let! order, _ =
                            AccountRequest<OrderResponse>
                                this
                                System.Net.Http.HttpMethod.Post
                                orderURL
                                None
                                [| System.Net.HttpStatusCode.OK |]

                        return! DriveOrder order

                    | "ready" ->
                        return
                            Ready
                                {|
                                    AccountURL = this.AccountURL
                                    OrderURL = orderURL
                                |}

                    | "valid" ->
                        let certificateURL =
                            match order.CertificateURL |> Option.ofObj with
                            | Some certificateURL -> certificateURL
                            | None -> failwith "Order does not have certificate URL"

                        let request = new System.Net.Http.HttpRequestMessage (System.Net.Http.HttpMethod.Get, certificateURL)

                        let! response =
                            Common.SendRequest
                                this.Client
                                request
                                [| System.Net.HttpStatusCode.OK |]
                                this.Log
                                this.CancellationToken

                        let! certificate = response.Content.ReadAsByteArrayAsync ()

                        return Valid certificate

                    | _ ->
                        failwith "Order has unexpected status"
                        return Unchecked.defaultof<_>
                }

            return! DriveOrder order
        }

    member internal this.EndOrder
        (orderURL: string)
        (pendingChallenge: EndOrderChallengeParameters option)
        (csr: byte array)
        : System.Threading.Tasks.Task<byte array> =
        FSharp.Control.Tasks.Builders.task {
            match pendingChallenge with
            | Some pendingChallenge ->
                this.Log.LogInformation ("Completing challenge {challengeURL} ...", pendingChallenge.ChallengeURL)

                let! challenge, _ =
                    AccountRequest<ChallengeResponse>
                        this
                        System.Net.Http.HttpMethod.Post
                        pendingChallenge.ChallengeURL
                        (Some (new ChallengeCompleteRequest () :> obj))
                        [| System.Net.HttpStatusCode.OK |]

                let rec DriveChallenge
                    (challenge: ChallengeResponse)
                    : System.Threading.Tasks.Task = FSharp.Control.Tasks.Builders.unitTask {
                        this.Log.LogInformation (
                            "{acmeObject} {acmeObjectURL} has {acmeObjectStatus} status",
                            "Challenge",
                            pendingChallenge.ChallengeURL,
                            challenge.Status
                        )

                        match challenge.Status with
                        | "pending"
                        | "processing" ->
                            let! () = 1.0 |> System.TimeSpan.FromSeconds |> System.Threading.Tasks.Task.Delay

                            let! challenge, _ =
                                AccountRequest<ChallengeResponse>
                                    this
                                    System.Net.Http.HttpMethod.Post
                                    pendingChallenge.ChallengeURL
                                    None
                                    [| System.Net.HttpStatusCode.OK |]

                            return! DriveChallenge challenge

                        | "valid" ->
                            ()

                        | _ ->
                            failwith "Challenge has unexpected status"
                    }

                let! () = DriveChallenge challenge

                this.Log.LogInformation ("Waiting for authorization {authorizationURL} ...", pendingChallenge.AuthorizationURL)

                let rec DriveAuthorization
                    ()
                    : System.Threading.Tasks.Task = FSharp.Control.Tasks.Builders.unitTask {
                        let! authorization, _ =
                            AccountRequest<AuthorizationResponse>
                                this
                                System.Net.Http.HttpMethod.Post
                                pendingChallenge.AuthorizationURL
                                None
                                [| System.Net.HttpStatusCode.OK |]

                        this.Log.LogInformation (
                            "{acmeObject} {acmeObjectURL} has {acmeObjectStatus} status",
                            "Authorization",
                            pendingChallenge.AuthorizationURL,
                            authorization.Status
                        )

                        match authorization.Status with
                        | "pending" ->
                            let! () = 1.0 |> System.TimeSpan.FromSeconds |> System.Threading.Tasks.Task.Delay

                            return! DriveAuthorization ()

                        | "valid" ->
                            ()

                        | _ ->
                            failwith "Authorization has unexpected status"
                    }

                let! () = DriveAuthorization ()

                ()

            | None ->
                ()

            this.Log.LogInformation ("Waiting for order {orderURL} ...", orderURL)

            let! order, _ =
                AccountRequest<OrderResponse>
                    this
                    System.Net.Http.HttpMethod.Post
                    orderURL
                    None
                    [| System.Net.HttpStatusCode.OK |]

            let rec DriveOrder
                (order: OrderResponse)
                : System.Threading.Tasks.Task<byte array> =
                FSharp.Control.Tasks.Builders.task {
                    this.Log.LogInformation (
                        "{acmeObject} {acmeObjectURL} has {acmeObjectStatus} status",
                        "Order",
                        orderURL,
                        order.Status
                    )

                    match order.Status with
                    | "processing" ->
                        let! () = 1.0 |> System.TimeSpan.FromSeconds |> System.Threading.Tasks.Task.Delay

                        let! order, _ =
                            AccountRequest<OrderResponse>
                                this
                                System.Net.Http.HttpMethod.Post
                                orderURL
                                None
                                [| System.Net.HttpStatusCode.OK |]

                        return! DriveOrder order

                    | "ready" ->
                        let finalizeURL =
                            match order.FinalizeURL |> Option.ofObj with
                            | Some finalizeURL -> finalizeURL
                            | None -> failwith "Order does not have finalize URL"

                        let! order, _ =
                            AccountRequest<OrderResponse>
                                this
                                System.Net.Http.HttpMethod.Post
                                finalizeURL
                                (Some ({
                                    FinalizeOrderRequest.Csr = (csr |> ConvertBytesToBase64UrlString)
                                } :> obj))
                                [| System.Net.HttpStatusCode.OK |]

                        return! DriveOrder order

                    | "valid" ->
                        let certificateURL =
                            match order.CertificateURL |> Option.ofObj with
                            | Some certificateURL -> certificateURL
                            | None -> failwith "Order does not have certificate URL"

                        let request = new System.Net.Http.HttpRequestMessage (System.Net.Http.HttpMethod.Get, certificateURL)

                        let! response =
                            Common.SendRequest
                                this.Client
                                request
                                [| System.Net.HttpStatusCode.OK |]
                                this.Log
                                this.CancellationToken

                        let! certificate = response.Content.ReadAsByteArrayAsync ()

                        return certificate

                    | _ ->
                        failwith "Order has unexpected status"
                        return Unchecked.defaultof<_>
                }

            return! DriveOrder order
        }
