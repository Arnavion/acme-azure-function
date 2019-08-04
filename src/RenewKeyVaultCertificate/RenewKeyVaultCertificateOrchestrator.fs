namespace acme_azure_function

open Microsoft.Extensions.Logging

module RenewKeyVaultCertificateOrchestratorManager =
    [<Microsoft.Azure.WebJobs.FunctionName("RenewKeyVaultCertificateOrchestratorManager")>]
    [<Microsoft.Azure.WebJobs.Singleton>]
    let Run
            ([<Microsoft.Azure.WebJobs.TimerTrigger("0 0 0 * * *")>] timerInfo: Microsoft.Azure.WebJobs.TimerInfo)
            // ([<Microsoft.Azure.WebJobs.HttpTrigger("Get")>] request: obj)
            ([<Microsoft.Azure.WebJobs.OrchestrationClient>] client: Microsoft.Azure.WebJobs.DurableOrchestrationClient)
            (log: Microsoft.Extensions.Logging.ILogger)
        : System.Threading.Tasks.Task =
        Common.Function "RenewKeyVaultCertificateOrchestratorManager" log (fun () -> FSharp.Control.Tasks.Builders.task {
            let _ = timerInfo
            // let _ = request

            log.LogInformation "Starting instance..."
            let! instanceID = client.StartNewAsync ("RenewKeyVaultCertificateOrchestratorInstance", None |> Option.toObj)
            log.LogInformation ("Started instance {instanceID}", instanceID)
            return ()
        }) :> _

module RenewKeyVaultCertificateOrchestratorInstance =
    [<Microsoft.Azure.WebJobs.FunctionName("RenewKeyVaultCertificateOrchestratorInstance")>]
    [<Microsoft.Azure.WebJobs.Singleton>]
    let Run
        ([<Microsoft.Azure.WebJobs.OrchestrationTrigger>] context: Microsoft.Azure.WebJobs.DurableOrchestrationContext)
        (log: Microsoft.Extensions.Logging.ILogger)
        : System.Threading.Tasks.Task =
        Common.Function
            (sprintf "RenewKeyVaultCertificateOrchestratorInstance %s:%s" context.InstanceId (context.CurrentUtcDateTime.ToString "o"))
            log
            (fun () -> FSharp.Control.Tasks.Builders.task {
                let! checkCertificateResponse =
                    context.CallActivityAsync<CheckKeyVaultCertificate.Response> (
                        "CheckKeyVaultCertificate",
                        {
                            CheckKeyVaultCertificate.Request.SubscriptionID = Settings.Instance.AzureSubscriptionID
                            CheckKeyVaultCertificate.Request.ResourceGroupName = Settings.Instance.AzureResourceGroupName
                            CheckKeyVaultCertificate.Request.AzureAuth = Settings.Instance.AzureAuth
                            CheckKeyVaultCertificate.Request.KeyVaultName = Settings.Instance.AzureKeyVaultName
                            CheckKeyVaultCertificate.Request.KeyVaultCertificateName = Settings.Instance.AzureKeyVaultCertificateName
                        }
                    )

                let needNewCertificate =
                    match checkCertificateResponse.CertificateExpiry with
                    | Some certificateExpiry ->
                        certificateExpiry < (context.CurrentUtcDateTime + (System.TimeSpan.FromDays 30.0))
                    | None ->
                        true

                if needNewCertificate then
                    let getAcmeAccountKeyResponse =
                        context.CallActivityAsync<GetAcmeAccountKey.Response> (
                            "GetAcmeAccountKey",
                            {
                                GetAcmeAccountKey.Request.SubscriptionID = Settings.Instance.AzureSubscriptionID
                                GetAcmeAccountKey.Request.ResourceGroupName = Settings.Instance.AzureResourceGroupName
                                GetAcmeAccountKey.Request.AzureAuth = Settings.Instance.AzureAuth
                                GetAcmeAccountKey.Request.KeyVaultName = Settings.Instance.AzureKeyVaultName
                                GetAcmeAccountKey.Request.AccountKeySecretName = Settings.Instance.AcmeAccountKeySecretName
                            }
                        )

                    let beginAcmeOrderResponse = FSharp.Control.Tasks.Builders.task {
                        let! getAcmeAccountKeyResponse = getAcmeAccountKeyResponse

                        return!
                            context.CallActivityAsync<BeginAcmeOrder.Response> (
                                "BeginAcmeOrder",
                                {
                                    BeginAcmeOrder.Request.AccountKey = getAcmeAccountKeyResponse
                                    BeginAcmeOrder.Request.DirectoryURL = Settings.Instance.AcmeDirectoryURL
                                    BeginAcmeOrder.Request.ContactURL = Settings.Instance.AcmeContactURL
                                    BeginAcmeOrder.Request.DomainName = Settings.Instance.DomainName
                                }
                            )
                    }


                    let createCsrResponse =
                        context.CallActivityAsync<CreateCsr.Response> (
                            "CreateCsr",
                            {
                                CreateCsr.Request.DomainName = Settings.Instance.DomainName
                            }
                        )


                    let! newCertificate, deleteChallengeBlobFromStorageAccountResponse = FSharp.Control.Tasks.Builders.task {
                        match! beginAcmeOrderResponse with
                        | BeginAcmeOrder.Response.Pending pending ->
                            let! () =
                                context.CallActivityAsync<SetChallengeBlobInStorageAccount.Response> (
                                    "SetChallengeBlobInStorageAccount",
                                    {
                                        SetChallengeBlobInStorageAccount.Request.SubscriptionID = Settings.Instance.AzureSubscriptionID
                                        SetChallengeBlobInStorageAccount.Request.ResourceGroupName = Settings.Instance.AzureResourceGroupName
                                        SetChallengeBlobInStorageAccount.Request.AzureAuth = Settings.Instance.AzureAuth
                                        SetChallengeBlobInStorageAccount.Request.StorageAccountName = Settings.Instance.AzureStorageAccountName
                                        SetChallengeBlobInStorageAccount.Request.Action =
                                            SetChallengeBlobInStorageAccount.Action.Create (
                                                pending.ChallengeBlobPath,
                                                pending.ChallengeBlobContent
                                            )
                                    }
                                )

                            let! getAcmeAccountKeyResponse = getAcmeAccountKeyResponse

                            let! createCsrResponse = createCsrResponse

                            let! endAcmeOrderResponse = FSharp.Control.Tasks.Builders.task {
                                try
                                    let! endAcmeOrderResponse =
                                        context.CallActivityAsync<EndAcmeOrder.Response> (
                                            "EndAcmeOrder",
                                            {
                                                EndAcmeOrder.Request.AccountKey = getAcmeAccountKeyResponse
                                                EndAcmeOrder.Request.AccountURL = pending.AccountURL
                                                EndAcmeOrder.Request.DirectoryURL = Settings.Instance.AcmeDirectoryURL
                                                EndAcmeOrder.Request.OrderURL = pending.OrderURL
                                                EndAcmeOrder.Request.PendingChallenge =
                                                    Some ({ AuthorizationURL = pending.AuthorizationURL; ChallengeURL = pending.ChallengeURL })
                                                EndAcmeOrder.Request.Csr = createCsrResponse.Csr
                                            }
                                        )

                                    return Ok endAcmeOrderResponse.Certificate
                                with ex ->
                                    return Error ex
                            }

                            let deleteChallengeBlobFromStorageAccountResponse =
                                context.CallActivityAsync<SetChallengeBlobInStorageAccount.Response> (
                                    "SetChallengeBlobInStorageAccount",
                                    {
                                        SetChallengeBlobInStorageAccount.Request.SubscriptionID = Settings.Instance.AzureSubscriptionID
                                        SetChallengeBlobInStorageAccount.Request.ResourceGroupName = Settings.Instance.AzureResourceGroupName
                                        SetChallengeBlobInStorageAccount.Request.AzureAuth = Settings.Instance.AzureAuth
                                        SetChallengeBlobInStorageAccount.Request.StorageAccountName = Settings.Instance.AzureStorageAccountName
                                        SetChallengeBlobInStorageAccount.Request.Action = SetChallengeBlobInStorageAccount.Action.Delete pending.ChallengeBlobPath
                                    }
                                )

                            return endAcmeOrderResponse, Some deleteChallengeBlobFromStorageAccountResponse

                        | BeginAcmeOrder.Response.Ready ready ->
                            let! getAcmeAccountKeyResponse = getAcmeAccountKeyResponse

                            let! createCsrResponse = createCsrResponse

                            let! endAcmeOrderResponse =
                                context.CallActivityAsync<EndAcmeOrder.Response> (
                                    "EndAcmeOrder",
                                    {
                                        EndAcmeOrder.Request.AccountKey = getAcmeAccountKeyResponse
                                        EndAcmeOrder.Request.AccountURL = ready.AccountURL
                                        EndAcmeOrder.Request.DirectoryURL = Settings.Instance.AcmeDirectoryURL
                                        EndAcmeOrder.Request.OrderURL = ready.OrderURL
                                        EndAcmeOrder.Request.PendingChallenge = None
                                        EndAcmeOrder.Request.Csr = createCsrResponse.Csr
                                    }
                                )

                            return Ok endAcmeOrderResponse.Certificate, None

                        | BeginAcmeOrder.Response.Valid certificate ->
                            return Ok certificate, None
                    }


                    match newCertificate with
                    | Ok newCertificate ->
                        let setNewKeyVaultCertificate = FSharp.Control.Tasks.Builders.task {
                            let! createCsrResponse = createCsrResponse

                            let! () =
                                context.CallActivityAsync<SetNewKeyVaultCertificate.Response> (
                                    "SetNewKeyVaultCertificate",
                                    {
                                        SetNewKeyVaultCertificate.Request.SubscriptionID = Settings.Instance.AzureSubscriptionID
                                        SetNewKeyVaultCertificate.Request.ResourceGroupName = Settings.Instance.AzureResourceGroupName
                                        SetNewKeyVaultCertificate.Request.AzureAuth = Settings.Instance.AzureAuth
                                        SetNewKeyVaultCertificate.Request.Certificate = newCertificate
                                        SetNewKeyVaultCertificate.Request.CertificatePrivateKey = createCsrResponse.PrivateKey
                                        SetNewKeyVaultCertificate.Request.DomainName = Settings.Instance.DomainName
                                        SetNewKeyVaultCertificate.Request.KeyVaultName = Settings.Instance.AzureKeyVaultName
                                        SetNewKeyVaultCertificate.Request.KeyVaultCertificateName = Settings.Instance.AzureKeyVaultCertificateName
                                    }
                                )

                            return ()
                        }

                        let! () = setNewKeyVaultCertificate

                        ()
                    | Error _ -> ()

                    match deleteChallengeBlobFromStorageAccountResponse with
                    | Some deleteChallengeBlobFromStorageAccountResponse ->
                        let! () = deleteChallengeBlobFromStorageAccountResponse
                        ()
                    | None -> ()

                    match newCertificate with
                    | Ok _ -> ()
                    | Error err -> raise err

                    log.LogInformation "Certificate has been renewed"
                else
                    log.LogInformation "Certificate does not need to be renewed"

                return ()
            }) :> _
