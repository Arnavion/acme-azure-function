module internal ArnavionDev.AzureFunctions.UpdateCdnCertificateFunction.Settings

[<Struct; System.Runtime.CompilerServices.IsReadOnly; System.Runtime.Serialization.DataContract>]
type RawSettings = {
    // The Azure subscription ID
    [<field: System.Runtime.Serialization.DataMember(Name = "AzureSubscriptionID")>]
    AzureSubscriptionID: string

    // The name of the Azure resource group
    [<field: System.Runtime.Serialization.DataMember(Name = "AzureResourceGroupName")>]
    AzureResourceGroupName: string

    // The name of the Azure CDN profile
    [<field: System.Runtime.Serialization.DataMember(Name = "AzureCdnProfileName")>]
    AzureCdnProfileName: string

    // The name of the Azure CDN endpoint in the Azure CDN profile
    [<field: System.Runtime.Serialization.DataMember(Name = "AzureCdnEndpointName")>]
    AzureCdnEndpointName: string

    // The name of the custom domain resource in the Azure CDN endpoint
    [<field: System.Runtime.Serialization.DataMember(Name = "AzureCdnCustomDomainName")>]
    AzureCdnCustomDomainName: string

    // The name of the Azure KeyVault
    [<field: System.Runtime.Serialization.DataMember(Name = "AzureKeyVaultName")>]
    AzureKeyVaultName: string

    // The name of the certificate in the Azure KeyVault that contains the TLS certificate.
    // The new certificate will be uploaded here, and used for the custom domain.
    [<field: System.Runtime.Serialization.DataMember(Name = "AzureKeyVaultCertificateName")>]
    AzureKeyVaultCertificateName: string

    // The application ID of the service principal that this function should use to access Azure resources.
    // Only needed for local testing; the final released function should be set to use the function app MSI.
    [<field: System.Runtime.Serialization.DataMember(Name = "AzureClientID")>]
    AzureClientID: string

    // The password of the service principal that this function should use to access Azure resources.
    // Only needed for local testing; the final released function should be set to use the function app MSI.
    [<field: System.Runtime.Serialization.DataMember(Name = "AzureClientSecret")>]
    AzureClientSecret: string

    // The tenant ID of the service principal that this function should use to access Azure resources.
    // Only needed for local testing; the final released function should be set to use the function app MSI.
    [<field: System.Runtime.Serialization.DataMember(Name = "AzureTenantID")>]
    AzureTenantID: string
}

let Instance =
    let rawSettings = "SECRET_SETTINGS" |> System.Environment.GetEnvironmentVariable |> Option.ofObj
    let rawSettings =
        match rawSettings with
        | Some rawSettings -> Newtonsoft.Json.JsonConvert.DeserializeObject<RawSettings> (rawSettings)
        | None -> failwith "SECRET_SETTINGS env var is not set"

    let azureAuth =
        ArnavionDev.AzureFunctions.RestAPI.Azure.GetAuth
            (rawSettings.AzureClientID |> Option.ofObj)
            (rawSettings.AzureClientSecret |> Option.ofObj)
            (rawSettings.AzureTenantID |> Option.ofObj)

    {|
        AzureSubscriptionID = rawSettings.AzureSubscriptionID
        AzureResourceGroupName = rawSettings.AzureResourceGroupName

        AzureAuth = azureAuth

        AzureCdnProfileName = rawSettings.AzureCdnProfileName
        AzureCdnEndpointName = rawSettings.AzureCdnEndpointName
        AzureCdnCustomDomainName = rawSettings.AzureCdnCustomDomainName

        AzureKeyVaultName = rawSettings.AzureKeyVaultName
        AzureKeyVaultCertificateName = rawSettings.AzureKeyVaultCertificateName
    |}
