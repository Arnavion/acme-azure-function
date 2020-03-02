module ArnavionDev.AzureFunctions.RestAPI.Azure

open Microsoft.Extensions.Logging

type Auth =
| ManagedIdentity of Endpoint: string * Secret: string
| ServicePrincipal of ClientID: string * ClientSecret: string * TenantID: string

let GetAuth
    (clientID: string option)
    (clientSecret: string option)
    (tenantID: string option)
    : Auth =
    let azureAuth =
        (
            "MSI_ENDPOINT" |> System.Environment.GetEnvironmentVariable |> Option.ofObj,
            "MSI_SECRET" |> System.Environment.GetEnvironmentVariable |> Option.ofObj
        )
        ||> Option.map2 (fun msiEndpoint msiSecret -> Auth.ManagedIdentity (msiEndpoint, msiSecret))
        |> Option.orElseWith (fun () ->
            (clientID, clientSecret, tenantID)
            |||> Option.map3 (fun clientID clientSecret tenantID -> Auth.ServicePrincipal (clientID, clientSecret, tenantID))
        )
    match azureAuth with
    | Some azureAuth -> azureAuth
    | None -> failwith "Found neither MSI_ENDPOINT+MSI_SECRET nor AzureClientID+AzureClientSecret+AzureTenantID"

[<Struct; System.Runtime.Serialization.DataContract>]
type private TokenResponse = {
    [<field:System.Runtime.Serialization.DataMember(Name = "access_token")>]
    AccessToken: string
    [<field:System.Runtime.Serialization.DataMember(Name = "token_type")>]
    TokenType: string
}

let private GetAuthorization
    (auth: Auth)
    (resource: string)

    (client: System.Net.Http.HttpClient)
    (serializer: Newtonsoft.Json.JsonSerializer)
    (log: Microsoft.Extensions.Logging.ILogger)
    (cancellationToken: System.Threading.CancellationToken)
    : System.Threading.Tasks.Task<System.Net.Http.Headers.AuthenticationHeaderValue> =
    FSharp.Control.Tasks.Builders.task {
        let request =
            match auth with
            | ManagedIdentity (endpoint, secret) ->
                let request =
                    new System.Net.Http.HttpRequestMessage (
                        System.Net.Http.HttpMethod.Get,
                        (sprintf "%s?resource=%s&api-version=2017-09-01" endpoint resource)
                    )
                request.Headers.Add ("Secret", secret)
                request
            | ServicePrincipal (clientID, clientSecret, tenantID) ->
                let request =
                    new System.Net.Http.HttpRequestMessage (
                        System.Net.Http.HttpMethod.Post,
                        (sprintf "https://login.microsoftonline.com/%s/oauth2/token" tenantID)
                    )
                request.Content <-
                    new System.Net.Http.FormUrlEncodedContent (
                        dict [
                            ("grant_type", "client_credentials");
                            ("client_id", clientID);
                            ("client_secret", clientSecret);
                            ("resource", resource);
                        ]
                    )
                request

        let! response =
            ArnavionDev.AzureFunctions.Common.SendRequest
                client
                request
                [| System.Net.HttpStatusCode.OK |]
                log
                cancellationToken

        let! tokenResponse =
            ArnavionDev.AzureFunctions.Common.Deserialize
                serializer
                response
                [| (System.Net.HttpStatusCode.OK, typedefof<TokenResponse>) |]
                log
                cancellationToken
        let tokenResponse =
            match tokenResponse with
            | System.Net.HttpStatusCode.OK, (:? TokenResponse as tokenResponse) -> tokenResponse
            | _ -> failwith "unreachable"
        return new System.Net.Http.Headers.AuthenticationHeaderValue (tokenResponse.TokenType, tokenResponse.AccessToken)
    }


type Account
    (
        subscriptionID: string,
        resourceGroupName: string,
        auth: Auth,
        log: Microsoft.Extensions.Logging.ILogger,
        cancellationToken: System.Threading.CancellationToken
    ) =
    static let UnixEpoch = new System.DateTime (1970, 1, 1, 0, 0, 0, 0, System.DateTimeKind.Utc)

    let client = new System.Net.Http.HttpClient ()
    let serializer = new Newtonsoft.Json.JsonSerializer ()

    let managementAuthorization = lazy FSharp.Control.Tasks.Builders.task {
        log.LogInformation "Getting Management API authorization..."
        let! managementAuthorization =
            GetAuthorization
                auth
                "https://management.azure.com"
                client
                serializer
                log
                cancellationToken
        log.LogInformation "Got Management API authorization"
        return managementAuthorization
    }

    let keyVaultAuthorization = lazy FSharp.Control.Tasks.Builders.task {
        log.LogInformation "Getting KeyVault API authorization..."
        let! keyVaultAuthorization =
            GetAuthorization
                auth
                "https://vault.azure.net"
                client
                serializer
                log
                cancellationToken
        log.LogInformation "Got KeyVault API authorization"
        return keyVaultAuthorization
    }

    member this.GetCdnCustomDomainCertificate
        (cdnProfileName: string)
        (cdnEndpointName: string)
        (cdnCustomDomainName: string)
        : System.Threading.Tasks.Task<string option> =
        FSharp.Control.Tasks.Builders.task {
            let! getCustomDomainResponse =
                this.Request
                    System.Net.Http.HttpMethod.Get
                    (this.ManagementRequestParameters (sprintf
                        "/providers/Microsoft.Cdn/profiles/%s/endpoints/%s/customDomains/%s?api-version=2018-04-02"
                        cdnProfileName
                        cdnEndpointName
                        cdnCustomDomainName
                    ))
                    None
                    [| (System.Net.HttpStatusCode.OK, typedefof<CdnCustomDomainResponse>) |]

            return
                match getCustomDomainResponse with
                | System.Net.HttpStatusCode.OK, (:? CdnCustomDomainResponse as getCustomDomainResponse) ->
                    getCustomDomainResponse.Properties.CustomHttpsParameters
                    |> Option.ofNullable
                    |> Option.map (fun customHttpsParameters -> customHttpsParameters.CertificateSourceParameters.SecretVersion)
                | _ ->
                    failwith "unreachable"
        }

    member this.SetCdnCustomDomainCertificate
        (cdnProfileName: string)
        (cdnEndpointName: string)
        (cdnCustomDomainName: string)
        (keyVaultName: string)
        (secretName: string)
        (secretVersion: string)
        : System.Threading.Tasks.Task =
        FSharp.Control.Tasks.Builders.unitTask {
            let! _ =
                this.Request
                    System.Net.Http.HttpMethod.Post
                    (this.ManagementRequestParameters (sprintf
                        "/providers/Microsoft.Cdn/profiles/%s/endpoints/%s/customDomains/%s/enableCustomHttps?api-version=2018-04-02"
                        cdnProfileName
                        cdnEndpointName
                        cdnCustomDomainName
                    ))
                    (Some ({
                        CdnCustomDomain_Properties_CustomHttpsParameters.CertificateSource = "AzureKeyVault"
                        CdnCustomDomain_Properties_CustomHttpsParameters.CertificateSourceParameters = {
                            CdnCustomDomain_Properties_CustomHttpsParameters_CertificateSourceParameters.DeleteRule = "NoAction"
                            CdnCustomDomain_Properties_CustomHttpsParameters_CertificateSourceParameters.KeyVaultName = keyVaultName
                            CdnCustomDomain_Properties_CustomHttpsParameters_CertificateSourceParameters.ODataType =
                                "#Microsoft.Azure.Cdn.Models.KeyVaultCertificateSourceParameters"
                            CdnCustomDomain_Properties_CustomHttpsParameters_CertificateSourceParameters.ResourceGroup = resourceGroupName
                            CdnCustomDomain_Properties_CustomHttpsParameters_CertificateSourceParameters.SecretName = secretName
                            CdnCustomDomain_Properties_CustomHttpsParameters_CertificateSourceParameters.SecretVersion = secretVersion
                            CdnCustomDomain_Properties_CustomHttpsParameters_CertificateSourceParameters.SubscriptionID = subscriptionID
                            CdnCustomDomain_Properties_CustomHttpsParameters_CertificateSourceParameters.UpdateRule = "NoAction"
                        }
                        CdnCustomDomain_Properties_CustomHttpsParameters.ProtocolType = "ServerNameIndication"
                     } :> obj))
                    [| (System.Net.HttpStatusCode.OK, typedefof<ArnavionDev.AzureFunctions.Common.Empty>) |]

            return ()
        }

    member this.GetKeyVaultCertificate
        (keyVaultName: string)
        (certificateName: string)
        : System.Threading.Tasks.Task<KeyVaultCertificate option> =
        FSharp.Control.Tasks.Builders.task {
            let! getKeyVaultCertificateResponse =
                this.Request
                    System.Net.Http.HttpMethod.Get
                    (this.KeyVaultRequestParameters keyVaultName (sprintf "/certificates/%s?api-version=2016-10-01" certificateName))
                    None
                    [|
                        (System.Net.HttpStatusCode.NotFound, typedefof<ArnavionDev.AzureFunctions.Common.Empty>)
                        (System.Net.HttpStatusCode.OK, typedefof<GetKeyVaultCertificateResponse>)
                    |]

            return
                match getKeyVaultCertificateResponse with
                | System.Net.HttpStatusCode.NotFound, _ ->
                    None

                | System.Net.HttpStatusCode.OK, (:? GetKeyVaultCertificateResponse as getKeyVaultCertificateResponse) ->
                    let expiry = UnixEpoch + (System.TimeSpan.FromSeconds (getKeyVaultCertificateResponse.Attributes.Expiry |> float))
                    let version = (getKeyVaultCertificateResponse.ID.Split '/') |> Seq.last

                    Some {
                        Expiry = expiry
                        Version = version
                    }

                | _ ->
                    failwith "unreachable"
        }

    member this.SetKeyVaultCertificate
        (keyVaultName: string)
        (certificateName: string)
        (certificateBytes: byte array)
        : System.Threading.Tasks.Task =
        FSharp.Control.Tasks.Builders.unitTask {
            let! _ =
                this.Request
                    System.Net.Http.HttpMethod.Post
                    (this.KeyVaultRequestParameters keyVaultName (sprintf "/certificates/%s/import?api-version=2016-10-01" certificateName))
                    (Some ({
                        SetKeyVaultCertificateRequest.Value = (certificateBytes |> System.Convert.ToBase64String)
                     } :> obj))
                    [| (System.Net.HttpStatusCode.OK, typedefof<ArnavionDev.AzureFunctions.Common.Empty>) |]

            return ()
        }

    member this.GetKeyVaultSecret
        (keyVaultName: string)
        (secretName: string)
        : System.Threading.Tasks.Task<byte array option> =
        FSharp.Control.Tasks.Builders.task {
            let! getKeyVaultSecretResponse =
                this.Request
                    System.Net.Http.HttpMethod.Get
                    (this.KeyVaultRequestParameters keyVaultName (sprintf "/secrets/%s?api-version=2016-10-01" secretName))
                    None
                    [|
                        (System.Net.HttpStatusCode.NotFound, typedefof<ArnavionDev.AzureFunctions.Common.Empty>)
                        (System.Net.HttpStatusCode.OK, typedefof<GetSetKeyVaultSecret>)
                    |]

            return
                match getKeyVaultSecretResponse with
                | System.Net.HttpStatusCode.NotFound, _ ->
                    None

                | System.Net.HttpStatusCode.OK, (:? GetSetKeyVaultSecret as getKeyVaultCertificateResponse) ->
                    getKeyVaultCertificateResponse.Value |> System.Convert.FromBase64String |> Some

                | _ ->
                    failwith "unreachable"
        }

    member this.SetKeyVaultSecret
        (keyVaultName: string)
        (secretName: string)
        (secretValue: byte array)
        : System.Threading.Tasks.Task =
        FSharp.Control.Tasks.Builders.unitTask {
            let! _ =
                this.Request
                    System.Net.Http.HttpMethod.Put
                    (this.KeyVaultRequestParameters keyVaultName (sprintf "/secrets/%s?api-version=2016-10-01" secretName))
                    (Some ({
                        GetSetKeyVaultSecret.ContentType = System.Net.Mime.MediaTypeNames.Application.Octet
                        GetSetKeyVaultSecret.Value = (secretValue |> System.Convert.ToBase64String)
                    } :> obj))
                    [| (System.Net.HttpStatusCode.OK, typedefof<GetSetKeyVaultSecret>) |]

            return ()
        }

    member this.SetDnsTxtRecord
        (dnsZoneName: string)
        (action: SetDnsTxtRecordAction)
        : System.Threading.Tasks.Task =
        FSharp.Control.Tasks.Builders.unitTask {
            let method, name, body, expectedResponses =
                match action with
                | Create (name, content) ->
                    System.Net.Http.HttpMethod.Put,
                    name,
                    (Some ({
                        CreateDnsRecordSet.Properties = {
                            CreateDnsRecordSet_Properties.TTL = 1
                            CreateDnsRecordSet_Properties.TXTRecords = [|
                                {
                                    CreateDnsRecordSet_Properties_TXTRecord.Value = [| content |]
                                }
                            |]
                        }
                    } :> obj)),
                    [| (System.Net.HttpStatusCode.Created, typedefof<ArnavionDev.AzureFunctions.Common.Empty>) |]
                | Delete name ->
                    System.Net.Http.HttpMethod.Delete,
                    name,
                    None,
                    [|
                        (System.Net.HttpStatusCode.Accepted, typedefof<ArnavionDev.AzureFunctions.Common.Empty>)
                        (System.Net.HttpStatusCode.NotFound, typedefof<ArnavionDev.AzureFunctions.Common.Empty>)
                        (System.Net.HttpStatusCode.OK, typedefof<ArnavionDev.AzureFunctions.Common.Empty>)
                    |]

            let! _ =
                this.Request
                    method
                    (this.ManagementRequestParameters (sprintf
                        "/providers/Microsoft.Network/dnsZones/%s/TXT/%s?api-version=2018-05-01"
                        dnsZoneName
                        name
                    ))
                    body
                    expectedResponses

            return ()
        }

    member internal __.ManagementRequestParameters
        (relativeURL: string)
        : string * Lazy<System.Threading.Tasks.Task<System.Net.Http.Headers.AuthenticationHeaderValue>> =
        let url =
            sprintf
                "https://management.azure.com/subscriptions/%s/resourceGroups/%s%s"
                subscriptionID
                resourceGroupName
                relativeURL
        url, managementAuthorization

    member internal __.KeyVaultRequestParameters
        (keyVaultName: string)
        (relativeURL: string)
        : string * Lazy<System.Threading.Tasks.Task<System.Net.Http.Headers.AuthenticationHeaderValue>> =
        let url = sprintf "https://%s.vault.azure.net%s" keyVaultName relativeURL
        url, keyVaultAuthorization

    member private __.Request
        (method: System.Net.Http.HttpMethod)
        ((url, authorization): string * Lazy<System.Threading.Tasks.Task<System.Net.Http.Headers.AuthenticationHeaderValue>>)
        (body: obj option)
        (expectedResponses: (System.Net.HttpStatusCode * System.Type) array)
        : System.Threading.Tasks.Task<(System.Net.HttpStatusCode * obj)> =
        FSharp.Control.Tasks.Builders.task {
            let request = new System.Net.Http.HttpRequestMessage (method, url)
            let! authorization = authorization.Value
            request.Headers.Authorization <- authorization

            body |> Option.iter (fun body ->
                ArnavionDev.AzureFunctions.Common.Serialize request serializer body ArnavionDev.AzureFunctions.Common.ApplicationJsonContentType)

            let! response =
                ArnavionDev.AzureFunctions.Common.SendRequest
                    client
                    request
                    (expectedResponses |> Seq.map (fun (statusCode, _) -> statusCode))
                    log
                    cancellationToken

            return!
                ArnavionDev.AzureFunctions.Common.Deserialize
                    serializer
                    response
                    expectedResponses
                    log
                    cancellationToken
        }


and KeyVaultCertificate = {
    Expiry: System.DateTime
    Version: string
}

and SetDnsTxtRecordAction =
| Create of Name: string * Content: string
| Delete of Name: string

and [<Struct; System.Runtime.Serialization.DataContract>] private CdnCustomDomainResponse = {
    [<field:System.Runtime.Serialization.DataMember(Name = "properties")>]
    Properties: CdnCustomDomain_Properties
}
and [<Struct; System.Runtime.Serialization.DataContract>] private CdnCustomDomain_Properties = {
    [<field:System.Runtime.Serialization.DataMember(Name = "customHttpsParameters")>]
    CustomHttpsParameters: System.Nullable<CdnCustomDomain_Properties_CustomHttpsParameters>
}
and [<Struct; System.Runtime.Serialization.DataContract>] private CdnCustomDomain_Properties_CustomHttpsParameters = {
    [<field:System.Runtime.Serialization.DataMember(Name = "certificateSource")>]
    CertificateSource: string
    [<field:System.Runtime.Serialization.DataMember(Name = "certificateSourceParameters")>]
    CertificateSourceParameters: CdnCustomDomain_Properties_CustomHttpsParameters_CertificateSourceParameters
    [<field:System.Runtime.Serialization.DataMember(Name = "protocolType")>]
    ProtocolType: string
}
and [<Struct; System.Runtime.Serialization.DataContract>] private CdnCustomDomain_Properties_CustomHttpsParameters_CertificateSourceParameters = {
    [<field:System.Runtime.Serialization.DataMember(Name = "deleteRule")>]
    DeleteRule: string
    [<field:System.Runtime.Serialization.DataMember(Name = "vaultName")>]
    KeyVaultName: string
    [<field:System.Runtime.Serialization.DataMember(Name = "@odata.type")>]
    ODataType: string
    [<field:System.Runtime.Serialization.DataMember(Name = "resourceGroupName")>]
    ResourceGroup: string
    [<field:System.Runtime.Serialization.DataMember(Name = "secretName")>]
    SecretName: string
    [<field:System.Runtime.Serialization.DataMember(Name = "secretVersion")>]
    SecretVersion: string
    [<field:System.Runtime.Serialization.DataMember(Name = "subscriptionId")>]
    SubscriptionID: string
    [<field:System.Runtime.Serialization.DataMember(Name = "updateRule")>]
    UpdateRule: string
}
and [<Struct; System.Runtime.Serialization.DataContract>] private GetKeyVaultCertificateResponse_Attributes = {
    [<field:System.Runtime.Serialization.DataMember(Name = "exp")>]
    Expiry: uint64
}
and [<Struct; System.Runtime.Serialization.DataContract>] private GetKeyVaultCertificateResponse = {
    [<field:System.Runtime.Serialization.DataMember(Name = "attributes")>]
    Attributes: GetKeyVaultCertificateResponse_Attributes
    [<field:System.Runtime.Serialization.DataMember(Name = "id")>]
    ID: string
}
and [<Struct; System.Runtime.Serialization.DataContract>] private SetKeyVaultCertificateRequest = {
    [<field:System.Runtime.Serialization.DataMember(Name = "value")>]
    Value: string
}
and [<Struct; System.Runtime.Serialization.DataContract>] private GetSetKeyVaultSecret = {
    [<field:System.Runtime.Serialization.DataMember(Name = "contentType")>]
    ContentType: string
    [<field:System.Runtime.Serialization.DataMember(Name = "value")>]
    Value: string
}
and [<Struct; System.Runtime.Serialization.DataContract>] private CreateDnsRecordSet = {
    [<field:System.Runtime.Serialization.DataMember(Name = "properties")>]
    Properties: CreateDnsRecordSet_Properties
}
and [<Struct; System.Runtime.Serialization.DataContract>] private CreateDnsRecordSet_Properties = {
    [<field:System.Runtime.Serialization.DataMember(Name = "TTL")>]
    TTL: int
    [<field:System.Runtime.Serialization.DataMember(Name = "TXTRecords")>]
    TXTRecords: CreateDnsRecordSet_Properties_TXTRecord array
}
and [<Struct; System.Runtime.Serialization.DataContract>] private CreateDnsRecordSet_Properties_TXTRecord = {
    [<field:System.Runtime.Serialization.DataMember(Name = "value")>]
    Value: string array
}
