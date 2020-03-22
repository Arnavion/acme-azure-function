module internal ArnavionDev.AzureFunctions.AcmeFunction.Settings

[<Struct; System.Runtime.CompilerServices.IsReadOnly; System.Runtime.Serialization.DataContract>]
type private RawSettings = {
    // The directory URL of the ACME server
    [<field: System.Runtime.Serialization.DataMember(Name = "AcmeDirectoryURL")>]
    AcmeDirectoryURL: string

    // The name of the KeyVault secret that contains the ACME account key.
    // A new key will be generated and uploaded if this secret does not already exist.
    [<field: System.Runtime.Serialization.DataMember(Name = "AcmeAccountKeySecretName")>]
    AcmeAccountKeySecretName: string

    // The contact URL of the ACME account
    [<field: System.Runtime.Serialization.DataMember(Name = "AcmeContactURL")>]
    AcmeContactURL: string

    // The Azure subscription ID
    [<field: System.Runtime.Serialization.DataMember(Name = "AzureSubscriptionID")>]
    AzureSubscriptionID: string

    // The name of the Azure resource group
    [<field: System.Runtime.Serialization.DataMember(Name = "AzureResourceGroupName")>]
    AzureResourceGroupName: string

    // The name of the Azure KeyVault
    [<field: System.Runtime.Serialization.DataMember(Name = "AzureKeyVaultName")>]
    AzureKeyVaultName: string

    // The name of the certificate in the Azure KeyVault that contains the TLS certificate.
    // The new certificate will be uploaded here, and used for the custom domain.
    [<field: System.Runtime.Serialization.DataMember(Name = "AzureKeyVaultCertificateName")>]
    AzureKeyVaultCertificateName: string

    // The domain name to request the TLS certificate for
    [<field: System.Runtime.Serialization.DataMember(Name = "TopLevelDomainName")>]
    TopLevelDomainName: string

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
        AcmeDirectoryURL = rawSettings.AcmeDirectoryURL
        AcmeAccountKeySecretName = rawSettings.AcmeAccountKeySecretName
        AcmeContactURL = rawSettings.AcmeContactURL

        AzureSubscriptionID = rawSettings.AzureSubscriptionID
        AzureResourceGroupName = rawSettings.AzureResourceGroupName

        AzureAuth = azureAuth

        AzureKeyVaultName = rawSettings.AzureKeyVaultName
        AzureKeyVaultCertificateName = rawSettings.AzureKeyVaultCertificateName

        TopLevelDomainName = rawSettings.TopLevelDomainName
    |}
