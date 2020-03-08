module internal ArnavionDev.AzureFunctions.AcmeFunction.Ocsp

open Microsoft.Extensions.Logging

// https://tools.ietf.org/html/rfc6960

let private OcspRequestContentType: System.Net.Http.Headers.MediaTypeHeaderValue =
    new System.Net.Http.Headers.MediaTypeHeaderValue ("application/ocsp-request")

let private OcspResponseContentType: System.Net.Http.Headers.MediaTypeHeaderValue =
    new System.Net.Http.Headers.MediaTypeHeaderValue ("application/ocsp-response")

let private OcspNonceOid: Asn1.ObjectIdentifier = "1.3.6.1.5.5.7.48.1.2"

let private OcspBasicOid: Asn1.ObjectIdentifier = "1.3.6.1.5.5.7.48.1.1"

let private SHA1Oid: Asn1.ObjectIdentifier = "1.3.14.3.2.26"

type private OcspRequest = {
    TbsRequest: TbsRequest
}

and private TbsRequest = {
    RequestList: Request list
    RequestExtensions: X509.Extension list
}

and private Request = {
    Cert: CertID
}

and private CertID = {
    HashAlgorithm: X509.AlgorithmIdentifier
    IssuerNameHash: byte array
    IssuerKeyHash: byte array
    SerialNumber: X509.CertificateSerialNumber
}

and internal OcspResponse = {
    ResponseStatus: OcspResponseStatus
    ResponseBytes: ResponseBytes
}

and internal OcspResponseStatus =
| Successful
| MalformedRequest
| InternalError
| TryLater
| SigRequired
| Unauthorized

and internal ResponseBytes = {
    ResponseType: Asn1.ObjectIdentifier
    Response: byte array
}

// and internal BasicOcspResponse = {
//     TbsResponseData: ResponseData
// }

// and internal ResponseData = {
//     ResponderID: ResponderID
//     ProducedAt: Asn1.GeneralizedTime
//     Responses: SingleResponse array
// }

// and internal SingleResponse = {
//     CertID: CertID
//     CertStatis: CertStatus
// }

// and internal CertStatus =
// | Good
// | Revoked
// | Unknown

and private Encodable =
| OcspRequest of OcspRequest
| TbsRequest of TbsRequest
| Request of Request
| CertID of CertID

let rec private Encode (value: Encodable): Asn1.Encodable =
    match value with
    | OcspRequest value ->
        Asn1.Encodable.Sequence [
            value.TbsRequest |> Encodable.TbsRequest |> Encode;
        ]

    | TbsRequest value ->
        let requests =
            value.RequestList
            |> List.map (fun request -> request |> Encodable.Request |> Encode)
            |> Asn1.Encodable.Sequence
        let extensions =
            value.RequestExtensions
            |> List.map (fun request -> request |> X509.Encodable.Extension |> X509.Encode)
            |> Asn1.Encodable.Sequence
            |> (fun value -> Asn1.Encodable.ContextSpecific (0x02uy, true, value))
        Asn1.Encodable.Sequence [
            requests;
            extensions;
        ]

    | Request value ->
        Asn1.Encodable.Sequence [
            value.Cert |> Encodable.CertID |> Encode
        ]

    | CertID value ->
        Asn1.Encodable.Sequence [
            value.HashAlgorithm |> X509.Encodable.AlgorithmIdentifier |> X509.Encode;
            value.IssuerNameHash |> Asn1.Encodable.OctetString;
            value.IssuerKeyHash |> Asn1.Encodable.OctetString;
            value.SerialNumber |> X509.Encodable.CertificateSerialNumber |> X509.Encode;
        ]

let rec internal (|AsOcspResponse|_|) (bytes: byte list): OcspResponse option =
    match bytes with
    | Asn1.AsSequence (AsOcspResponseStatus (responseStatus, Asn1.AsContextSpecific (0x00uy, true, AsResponseBytes (responseBytes, []), [])), []) ->
        Some {
            ResponseStatus = responseStatus
            ResponseBytes = responseBytes
        }
    | _ -> None

and private (|AsOcspResponseStatus|_|) (bytes: byte list): (OcspResponseStatus * byte list) option =
    match bytes with
    | Asn1.AsEnumerated (value, rest) ->
        try
            let value = value |> int
            match value with
            | 0 -> Some (OcspResponseStatus.Successful, rest)
            | 1 -> Some (OcspResponseStatus.MalformedRequest, rest)
            | 2 -> Some (OcspResponseStatus.InternalError, rest)
            | 3 -> Some (OcspResponseStatus.TryLater, rest)
            | 5 -> Some (OcspResponseStatus.SigRequired, rest)
            | 6 -> Some (OcspResponseStatus.Unauthorized, rest)
            | _ -> None
        with
        | :? System.OverflowException -> None
    | _ -> None

and private (|AsResponseBytes|_|) (bytes: byte list): (ResponseBytes * byte list) option =
    match bytes with
    | Asn1.AsSequence (Asn1.AsObjectIdentifier (responseType, Asn1.AsOctetString (responseBytes, [])), rest) ->
        Some ({
            ResponseType = responseType
            Response = responseBytes
        }, rest)
    | _ -> None

let private SendRequest
    (client: System.Net.Http.HttpClient)
    (rng: System.Security.Cryptography.RandomNumberGenerator)
    (certificate: System.Security.Cryptography.X509Certificates.X509Certificate2)
    (issuer: System.Security.Cryptography.X509Certificates.X509Certificate2)
    (uri: string)
    (log: Microsoft.Extensions.Logging.ILogger)
    (cancellationToken: System.Threading.CancellationToken)
    : System.Threading.Tasks.Task<OcspResponse> = FSharp.Control.Tasks.Builders.task {
        let request =
            let serialNumberString = certificate.SerialNumber
            let serialNumber = Array.zeroCreate (serialNumberString.Length / 2)
            for i in 0..(serialNumber.Length - 1) do
                serialNumber.[i] <-
                    System.Byte.Parse (
                        serialNumberString.[(i * 2)..(i * 2 + 1)],
                        System.Globalization.NumberStyles.AllowHexSpecifier
                    )
            let serialNumber = new System.ReadOnlySpan<byte> (serialNumber)
            let serialNumber = new System.Numerics.BigInteger (serialNumber, false, true)

            let nonce = Array.zeroCreate 16
            rng.GetBytes nonce

            let requestBody: OcspRequest = {
                TbsRequest = {
                    RequestList = [
                        {
                            Cert = {
                                HashAlgorithm = {
                                    Algorithm = SHA1Oid
                                }
                                IssuerNameHash =
                                    use hasher = System.Security.Cryptography.SHA1.Create ()
                                    issuer.SubjectName.RawData |> hasher.ComputeHash
                                IssuerKeyHash =
                                    use hasher = System.Security.Cryptography.SHA1.Create ()
                                    issuer.PublicKey.EncodedKeyValue.RawData |> hasher.ComputeHash
                                SerialNumber = serialNumber
                            }
                        }
                    ]
                    RequestExtensions = [
                        {
                            ExtensionID = OcspNonceOid
                            ExtensionValue = (nonce |> Asn1.Encodable.OctetString |> Asn1.Encode |> Array.ofList)
                        }
                    ]
                }
            }
            let requestBody = requestBody |> Encodable.OcspRequest |> Encode |> Asn1.Encode |> Array.ofList
            log.LogInformation ("Request body: {requestBody}", (requestBody |> System.Convert.ToBase64String))

            let request = new System.Net.Http.HttpRequestMessage (System.Net.Http.HttpMethod.Post, uri)
            request.Content <- new System.Net.Http.ByteArrayContent (requestBody)
            request.Content.Headers.ContentType <- OcspRequestContentType
            request

        let! response = client.SendAsync (request, System.Net.Http.HttpCompletionOption.ResponseHeadersRead, cancellationToken)
        let response: System.Net.Http.HttpResponseMessage = response

        if response.Content.Headers.ContentType <> OcspResponseContentType then
            failwith (sprintf "unexpected response content type %O" response.Content.Headers.ContentType)

        let! responseBytes = response.Content.ReadAsByteArrayAsync ()
        log.LogInformation ("Response body: {responseBody}", (responseBytes |> System.Convert.ToBase64String))
        let responseBytes = responseBytes |> List.ofArray
        let responseTyped =
            match responseBytes with
            | AsOcspResponse response -> response
            | _ -> failwith (sprintf "malformed OCSP response %A" responseBytes)

        if response.StatusCode <> System.Net.HttpStatusCode.OK then
            failwith (sprintf "OCSP response returned %O %O" response.StatusCode responseTyped)

        return responseTyped
    }

let internal Verify
    (client: System.Net.Http.HttpClient)
    (rng: System.Security.Cryptography.RandomNumberGenerator)
    (certificate: System.Security.Cryptography.X509Certificates.X509Certificate2)
    (issuer: System.Security.Cryptography.X509Certificates.X509Certificate2)
    (log: Microsoft.Extensions.Logging.ILogger)
    (cancellationToken: System.Threading.CancellationToken)
    : System.Threading.Tasks.Task<bool> = FSharp.Control.Tasks.Builders.task {
        if certificate.IssuerName.Name <> issuer.SubjectName.Name then
            failwith (sprintf "certificate has issuer name %s but issuer has subject name %s" certificate.IssuerName.Name issuer.SubjectName.Name)

        let ocspUris =
            certificate.Extensions
            |> Seq.cast<System.Security.Cryptography.X509Certificates.X509Extension>
            |> Seq.choose (fun extension -> if extension.Oid.Value = "1.3.6.1.5.5.7.1.1" then Some extension.RawData else None)
            |> Seq.collect X509.GetOcspUris
        let ocspUris = ocspUris.GetEnumerator ()

        let mutable response = None
        while response |> Option.isNone && ocspUris.MoveNext () do
            let uri = ocspUris.Current

            log.LogInformation (
                "Verifying {certificateSubjectName}, issued by {certificateIssuerName}, via {certificateOcspUri} ...",
                certificate.SubjectName.Name,
                certificate.IssuerName.Name,
                uri
            )

            try
                let! ocspResponse = SendRequest client rng certificate issuer uri log cancellationToken
                printfn "%O" ocspResponse

                // TODO
                response <- Some true
            with
            | ex -> log.LogWarning ("Verification failed: {exception}", ex)

        match response with
        | Some response -> return response
        | None ->
            log.LogInformation "No successful OCSP verifications. Treating certificate as valid."
            return true
    }
