module acme_azure_function.Azure

open Microsoft.Extensions.Logging

type Auth = {
    Endpoint: string
    Secret: string
}

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
            new System.Net.Http.HttpRequestMessage (
                System.Net.Http.HttpMethod.Get,
                (sprintf "%s?resource=%s&api-version=2017-09-01" auth.Endpoint resource)
            )
        request.Headers.Add ("Secret", auth.Secret)

        let! response =
            Common.SendRequest
                client
                request
                [| System.Net.HttpStatusCode.OK |]
                log
                cancellationToken

        let! tokenResponse =
            Common.Deserialize
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


type internal Account internal
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

    let storageAccountAuthorization = lazy FSharp.Control.Tasks.Builders.task {
        log.LogInformation "Getting Storage Account API authorization..."
        let! storageAccountAuthorization =
            GetAuthorization
                auth
                "https://storage.azure.com"
                client
                serializer
                log
                cancellationToken
        log.LogInformation "Got Storage Account API authorization"
        return storageAccountAuthorization
    }

    member internal this.GetCdnCustomDomainCertificate
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

    member internal this.SetCdnCustomDomainCertificate
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
                    [| (System.Net.HttpStatusCode.OK, typedefof<Common.Empty>) |]

            return ()
        }

    member internal this.GetKeyVaultCertificate
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
                        (System.Net.HttpStatusCode.NotFound, typedefof<Common.Empty>)
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

    member internal this.SetKeyVaultCertificate
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
                    [| (System.Net.HttpStatusCode.OK, typedefof<Common.Empty>) |]

            return ()
        }

    member internal this.GetKeyVaultSecret
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
                        (System.Net.HttpStatusCode.NotFound, typedefof<Common.Empty>)
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

    member internal this.SetKeyVaultSecret
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

    member internal this.SetStorageAccountEnableHttpAccess
        (storageAccountName: string)
        (enableHttp: bool)
        : System.Threading.Tasks.Task =
        FSharp.Control.Tasks.Builders.unitTask {
            let! _ =
                this.Request
                    System.Net.Http.HttpMethod.Patch
                    (this.ManagementRequestParameters (sprintf "/providers/Microsoft.Storage/storageAccounts/%s/?api-version=2018-11-01" storageAccountName))
                    (Some ({
                        SetEnableHttpAccessOnStorageAccountRequest.Properties = {
                            SetEnableHttpAccessOnStorageAccountRequest_Properties.SupportsHttpsTrafficOnly = not enableHttp
                        }
                    } :> obj))
                    [| (System.Net.HttpStatusCode.OK, typedefof<Common.Empty>) |]

            return ()
        }

    member internal __.SetStorageAccountBlob
        (storageAccountName: string)
        (action: StorageAccountBlobAction)
        : System.Threading.Tasks.Task =
        FSharp.Control.Tasks.Builders.unitTask {
            let! storageAccountAuthorization = storageAccountAuthorization.Value

            let method, path, blobTypeHeader, body, expectedStatusCodes =
                match action with
                | Create (path, content) ->
                    System.Net.Http.HttpMethod.Put,
                    path,
                    Some ("x-ms-blob-type", "BlockBlob"),
                    Some content,
                    [| System.Net.HttpStatusCode.Created |]
                | Delete path ->
                    System.Net.Http.HttpMethod.Delete,
                    path,
                    None,
                    None,
                    [| System.Net.HttpStatusCode.Accepted; System.Net.HttpStatusCode.NotFound; System.Net.HttpStatusCode.OK |]

            let url = sprintf "https://%s.blob.core.windows.net%s" storageAccountName path
            let request = new System.Net.Http.HttpRequestMessage (method, url)
            request.Headers.Authorization <- storageAccountAuthorization
            request.Headers.Date <- System.Nullable (new System.DateTimeOffset (System.DateTime.UtcNow))
            request.Headers.Add ("x-ms-version", "2018-03-28")
            blobTypeHeader |> Option.iter request.Headers.Add

            body |> Option.iter (fun body ->
                request.Content <- new System.Net.Http.ByteArrayContent (body)
                request.Content.Headers.ContentType <-
                    new System.Net.Http.Headers.MediaTypeHeaderValue (System.Net.Mime.MediaTypeNames.Application.Octet)
            )

            let! _ =
                Common.SendRequest
                    client
                    request
                    expectedStatusCodes
                    log
                    cancellationToken

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

            body |> Option.iter (fun body -> Common.Serialize request serializer body Common.ApplicationJsonContentType)

            let! response =
                Common.SendRequest
                    client
                    request
                    (expectedResponses |> Seq.map (fun (statusCode, _) -> statusCode))
                    log
                    cancellationToken

            return!
                Common.Deserialize
                    serializer
                    response
                    expectedResponses
                    log
                    cancellationToken
        }


and internal KeyVaultCertificate = {
    Expiry: System.DateTime
    Version: string
}

and internal StorageAccountBlobAction =
| Create of Path: string * Content: byte array
| Delete of Path: string

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
and [<Struct; System.Runtime.Serialization.DataContract>] private SetEnableHttpAccessOnStorageAccountRequest = {
    [<field:System.Runtime.Serialization.DataMember(Name = "properties")>]
    Properties: SetEnableHttpAccessOnStorageAccountRequest_Properties
}
and [<Struct; System.Runtime.Serialization.DataContract>] private SetEnableHttpAccessOnStorageAccountRequest_Properties = {
    [<field:System.Runtime.Serialization.DataMember(Name = "supportsHttpsTrafficOnly")>]
    SupportsHttpsTrafficOnly: bool
}
