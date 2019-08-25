module internal acme_azure_function.Settings

[<Struct; System.Runtime.CompilerServices.IsReadOnly; System.Runtime.Serialization.DataContract>]
type RawSettings = {
    // The domain name to request the TLS certificate for
    [<field: System.Runtime.Serialization.DataMember(Name = "domainName")>]
    DomainName: string

    // The directory URL of the ACME server
    [<field: System.Runtime.Serialization.DataMember(Name = "acmeDirectoryURL")>]
    AcmeDirectoryURL: string

    // The name of the KeyVault secret that contains the ACME account key.
    // A new key will be generated and uploaded if this secret does not already exist.
    [<field: System.Runtime.Serialization.DataMember(Name = "acmeAccountKeySecretName")>]
    AcmeAccountKeySecretName: string

    // The contact URL of the ACME account
    [<field: System.Runtime.Serialization.DataMember(Name = "acmeContactURL")>]
    AcmeContactURL: string

    // The Azure subscription ID
    [<field: System.Runtime.Serialization.DataMember(Name = "azureSubscriptionID")>]
    AzureSubscriptionID: string

    // The name of the Azure resource group
    [<field: System.Runtime.Serialization.DataMember(Name = "azureResourceGroupName")>]
    AzureResourceGroupName: string

    // The name of the Azure CDN profile
    [<field: System.Runtime.Serialization.DataMember(Name = "azureCdnProfileName")>]
    AzureCdnProfileName: string

    // The name of the Azure CDN endpoint in the Azure CDN profile
    [<field: System.Runtime.Serialization.DataMember(Name = "azureCdnEndpointName")>]
    AzureCdnEndpointName: string

    // The name of the custom domain resource in the Azure CDN endpoint
    [<field: System.Runtime.Serialization.DataMember(Name = "azureCdnCustomDomainName")>]
    AzureCdnCustomDomainName: string

    // The name of the Azure KeyVault
    [<field: System.Runtime.Serialization.DataMember(Name = "azureKeyVaultName")>]
    AzureKeyVaultName: string

    // The name of the certificate in the Azure KeyVault that contains the TLS certificate.
    // The new certificate will be uploaded here, and used for the custom domain.
    [<field: System.Runtime.Serialization.DataMember(Name = "azureKeyVaultCertificateName")>]
    AzureKeyVaultCertificateName: string

    // The name of the Azure storage account backing the website.
    // Challenge blobs requested by the ACME server will be placed in the `$web` container under this storage account.
    [<field: System.Runtime.Serialization.DataMember(Name = "azureStorageAccountName")>]
    AzureStorageAccountName: string
}

let Instance =
    let rawSettings = "SECRET_SETTINGS" |> System.Environment.GetEnvironmentVariable |> Option.ofObj
    let rawSettings =
        match rawSettings with
        | Some rawSettings -> Newtonsoft.Json.JsonConvert.DeserializeObject<RawSettings> (rawSettings)
        | None -> failwith "SECRET_SETTINGS env var is not set"

    let msiEndpoint = "MSI_ENDPOINT" |> System.Environment.GetEnvironmentVariable |> Option.ofObj
    let msiEndpoint =
        match msiEndpoint with
        | Some msiEndpoint -> msiEndpoint
        | None -> failwith "MSI_ENDPOINT env var is not set"

    let msiSecret = "MSI_SECRET" |> System.Environment.GetEnvironmentVariable |> Option.ofObj
    let msiSecret =
        match msiSecret with
        | Some msiSecret -> msiSecret
        | None -> failwith "MSI_SECRET env var is not set"

    {|
        DomainName = rawSettings.DomainName

        AcmeDirectoryURL = rawSettings.AcmeDirectoryURL

        AcmeAccountKeySecretName = rawSettings.AcmeAccountKeySecretName
        AcmeContactURL = rawSettings.AcmeContactURL

        AzureSubscriptionID = rawSettings.AzureSubscriptionID
        AzureResourceGroupName = rawSettings.AzureResourceGroupName

        AzureAuth = {
            Azure.Auth.Endpoint = msiEndpoint
            Azure.Auth.Secret = msiSecret
        }

        AzureCdnProfileName = rawSettings.AzureCdnProfileName
        AzureCdnEndpointName = rawSettings.AzureCdnEndpointName
        AzureCdnCustomDomainName = rawSettings.AzureCdnCustomDomainName

        AzureKeyVaultName = rawSettings.AzureKeyVaultName
        AzureKeyVaultCertificateName = rawSettings.AzureKeyVaultCertificateName

        AzureStorageAccountName = rawSettings.AzureStorageAccountName
    |}
