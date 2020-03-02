module ArnavionDev.AzureFunctions.AcmeFunction.CreateCsr

open Microsoft.Extensions.Logging

type Request = {
    TopLevelDomainName: string
}

type Response = {
    Csr: string
    PrivateKey: KeyParameters
}
and KeyParameters = {
    D: string
    DP: string
    DQ: string
    Exponent: string
    InverseQ: string
    Modulus: string
    P: string
    Q: string
}

[<Microsoft.Azure.WebJobs.FunctionName("CreateCsr")>]
[<Microsoft.Azure.WebJobs.Singleton>]
let Run
    ([<Microsoft.Azure.WebJobs.Extensions.DurableTask.ActivityTrigger>] request: Request)
    (log: Microsoft.Extensions.Logging.ILogger)
    : System.Threading.Tasks.Task<Response> =
    ArnavionDev.AzureFunctions.Common.Function "CreateCsr" log (fun () -> FSharp.Control.Tasks.Builders.task {
        let domainName = sprintf "*.%s" request.TopLevelDomainName

        log.LogInformation ("Creating CSR for {domainName} ...", domainName)

        let key = System.Security.Cryptography.RSA.Create 4096

        let csr =
            new System.Security.Cryptography.X509Certificates.CertificateRequest (
                new System.Security.Cryptography.X509Certificates.X500DistinguishedName (sprintf "CN=%s" domainName),
                key,
                System.Security.Cryptography.HashAlgorithmName.SHA256,
                System.Security.Cryptography.RSASignaturePadding.Pkcs1
            )
        let csr = csr.CreateSigningRequest () |> System.Convert.ToBase64String

        let keyParameters = key.ExportParameters true
        let keyParameters = {
            D = keyParameters.D |> System.Convert.ToBase64String
            DP = keyParameters.DP |> System.Convert.ToBase64String
            DQ = keyParameters.DQ |> System.Convert.ToBase64String
            Exponent = keyParameters.Exponent |> System.Convert.ToBase64String
            InverseQ = keyParameters.InverseQ |> System.Convert.ToBase64String
            Modulus = keyParameters.Modulus |> System.Convert.ToBase64String
            P = keyParameters.P |> System.Convert.ToBase64String
            Q = keyParameters.Q |> System.Convert.ToBase64String
        }

        log.LogInformation ("Created CSR for {domainName}", domainName)

        return {
            Csr = csr
            PrivateKey = keyParameters
        }
    })
