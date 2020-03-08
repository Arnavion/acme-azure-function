namespace ArnavionDev.AzureFunctions.AcmeFunction

open Microsoft.Extensions.Logging

module OrchestratorManager =
    [<Microsoft.Azure.WebJobs.FunctionName("OrchestratorManager")>]
    [<Microsoft.Azure.WebJobs.Singleton>]
    let Run
#if LOCAL
            ([<Microsoft.Azure.WebJobs.HttpTrigger("Get")>] request: obj)
#else
            ([<Microsoft.Azure.WebJobs.TimerTrigger("0 0 0 * * *")>] timerInfo: Microsoft.Azure.WebJobs.TimerInfo)
#endif
            ([<Microsoft.Azure.WebJobs.Extensions.DurableTask.DurableClient>] client: Microsoft.Azure.WebJobs.Extensions.DurableTask.IDurableOrchestrationClient)
            (log: Microsoft.Extensions.Logging.ILogger)
        : System.Threading.Tasks.Task =
        ArnavionDev.AzureFunctions.Common.Function "OrchestratorManager" log (fun () -> FSharp.Control.Tasks.Builders.task {
#if LOCAL
            let _ = request
#else
            let _ = timerInfo
#endif

            log.LogInformation "Starting instance..."
            let! instanceID = client.StartNewAsync ("OrchestratorInstance", "", None |> Option.toObj)
            log.LogInformation ("Started instance {instanceID}", instanceID)
            return ()
        }) :> _

module OrchestratorInstance =
    [<Microsoft.Azure.WebJobs.FunctionName("OrchestratorInstance")>]
    [<Microsoft.Azure.WebJobs.Singleton>]
    let Run
        ([<Microsoft.Azure.WebJobs.Extensions.DurableTask.OrchestrationTrigger>] context: Microsoft.Azure.WebJobs.Extensions.DurableTask.IDurableOrchestrationContext)
        (log: Microsoft.Extensions.Logging.ILogger)
        : System.Threading.Tasks.Task =
        ArnavionDev.AzureFunctions.Common.Function
            (sprintf "OrchestratorInstance %s:%s" context.InstanceId (context.CurrentUtcDateTime.ToString "o"))
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

                if not checkCertificateResponse.Valid then
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
                                    BeginAcmeOrder.Request.TopLevelDomainName = Settings.Instance.TopLevelDomainName
                                }
                            )
                    }


                    let createCsrResponse =
                        context.CallActivityAsync<CreateCsr.Response> (
                            "CreateCsr",
                            {
                                CreateCsr.Request.TopLevelDomainName = Settings.Instance.TopLevelDomainName
                            }
                        )


                    let! newCertificate, deleteDnsTxtRecordResponse = FSharp.Control.Tasks.Builders.task {
                        match! beginAcmeOrderResponse with
                        | BeginAcmeOrder.Response.Pending pending ->
                            let! () =
                                context.CallActivityAsync<SetDnsTxtRecord.Response> (
                                    "SetDnsTxtRecord",
                                    {
                                        SetDnsTxtRecord.Request.SubscriptionID = Settings.Instance.AzureSubscriptionID
                                        SetDnsTxtRecord.Request.ResourceGroupName = Settings.Instance.AzureResourceGroupName
                                        SetDnsTxtRecord.Request.AzureAuth = Settings.Instance.AzureAuth
                                        SetDnsTxtRecord.Request.TopLevelDomainName = Settings.Instance.TopLevelDomainName
                                        SetDnsTxtRecord.Request.Action =
                                            ArnavionDev.AzureFunctions.RestAPI.Azure.SetDnsTxtRecordAction.Create (
                                                "_acme-challenge",
                                                pending.DnsTxtRecordContent
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

                            let deleteDnsTxtRecordResponse =
                                context.CallActivityAsync<SetDnsTxtRecord.Response> (
                                    "SetDnsTxtRecord",
                                    {
                                        SetDnsTxtRecord.Request.SubscriptionID = Settings.Instance.AzureSubscriptionID
                                        SetDnsTxtRecord.Request.ResourceGroupName = Settings.Instance.AzureResourceGroupName
                                        SetDnsTxtRecord.Request.AzureAuth = Settings.Instance.AzureAuth
                                        SetDnsTxtRecord.Request.TopLevelDomainName = Settings.Instance.TopLevelDomainName
                                        SetDnsTxtRecord.Request.Action = ArnavionDev.AzureFunctions.RestAPI.Azure.SetDnsTxtRecordAction.Delete "_acme-challenge"
                                    }
                                )

                            return endAcmeOrderResponse, Some deleteDnsTxtRecordResponse

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
                                        SetNewKeyVaultCertificate.Request.TopLevelDomainName = Settings.Instance.TopLevelDomainName
                                        SetNewKeyVaultCertificate.Request.KeyVaultName = Settings.Instance.AzureKeyVaultName
                                        SetNewKeyVaultCertificate.Request.KeyVaultCertificateName = Settings.Instance.AzureKeyVaultCertificateName
                                    }
                                )

                            return ()
                        }

                        let! () = setNewKeyVaultCertificate

                        ()
                    | Error _ -> ()

                    match deleteDnsTxtRecordResponse with
                    | Some deleteDnsTxtRecordResponse ->
                        let! () = deleteDnsTxtRecordResponse
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
