module ArnavionDev.AzureFunctions.Common

open Microsoft.Extensions.Logging

type internal Empty () =
    class end

let ApplicationJsonContentType: System.Net.Http.Headers.MediaTypeHeaderValue =
    new System.Net.Http.Headers.MediaTypeHeaderValue (System.Net.Mime.MediaTypeNames.Application.Json)

let ApplicationJoseJsonContentType: System.Net.Http.Headers.MediaTypeHeaderValue =
    new System.Net.Http.Headers.MediaTypeHeaderValue ("application/jose+json")

let UTF8Encoding: System.Text.Encoding = new System.Text.UTF8Encoding (false) :> _

let Serialize
    (request: System.Net.Http.HttpRequestMessage)
    (serializer: Newtonsoft.Json.JsonSerializer)
    (body: obj)
    (contentType: System.Net.Http.Headers.MediaTypeHeaderValue)
    : unit =
    use stream = new System.IO.MemoryStream ()
    using (new System.IO.StreamWriter (stream, UTF8Encoding, 1024, true)) (fun streamWriter ->
        serializer.Serialize (streamWriter, body)
    )

    stream.Seek (0L, System.IO.SeekOrigin.Begin) |> ignore
    let content = stream.ToArray ()
    request.Content <- new System.Net.Http.ByteArrayContent (content)
    request.Content.Headers.ContentType <- contentType

let SendRequest
    (client: System.Net.Http.HttpClient)
    (request: System.Net.Http.HttpRequestMessage)
    (expectedStatusCodes: System.Net.HttpStatusCode seq)
    (log: Microsoft.Extensions.Logging.ILogger)
    (cancellationToken: System.Threading.CancellationToken)
    : System.Threading.Tasks.Task<System.Net.Http.HttpResponseMessage> =
    FSharp.Control.Tasks.Builders.task {
        let authorization = request.Headers.Authorization

        let! initialResponse =
            client.SendAsync (request, System.Net.Http.HttpCompletionOption.ResponseHeadersRead, cancellationToken)
        let mutable response = initialResponse

        let mutable haveLocation = true
        while response.StatusCode = System.Net.HttpStatusCode.Accepted && haveLocation do
            match response.Headers.Location |> Option.ofObj with
            | Some location ->
                log.LogInformation ("HTTP response: {statusCode} {location}", response.StatusCode, location)

                let! () = System.Threading.Tasks.Task.Delay (System.TimeSpan.FromSeconds 1.0)

                let operationRequest = new System.Net.Http.HttpRequestMessage (System.Net.Http.HttpMethod.Get, location)
                operationRequest.Headers.Authorization <- authorization
                let! newResponse = client.SendAsync (operationRequest, System.Net.Http.HttpCompletionOption.ResponseHeadersRead, cancellationToken)
                response <- newResponse

            | None ->
                haveLocation <- false

        if Seq.contains response.StatusCode expectedStatusCodes |> not then
            let! content = response.Content.ReadAsStringAsync ()
            failwith (sprintf "%O: %s" response.StatusCode content)

        return response
    }

let Deserialize
    (serializer: Newtonsoft.Json.JsonSerializer)
    (response: System.Net.Http.HttpResponseMessage)
    (expectedResponses: (System.Net.HttpStatusCode * System.Type) seq)
    (log: Microsoft.Extensions.Logging.ILogger)
    (cancellationToken: System.Threading.CancellationToken)
    : System.Threading.Tasks.Task<(System.Net.HttpStatusCode * obj)> =
    FSharp.Control.Tasks.Builders.task {
        let! contentStream = response.Content.ReadAsStreamAsync ()
        use memoryStream = new System.IO.MemoryStream ()
        let! () = contentStream.CopyToAsync (memoryStream, cancellationToken)

        let responseType =
            match expectedResponses |> Seq.tryFind (fun (statusCode, _) -> response.StatusCode = statusCode) with
            | Some (_, responseType) -> responseType
            | None ->
                memoryStream.Seek (0L, System.IO.SeekOrigin.Begin) |> ignore
                let contentString =
                    using (new System.IO.StreamReader (memoryStream, UTF8Encoding, false, 1024, true)) (fun streamReader ->
                        streamReader.ReadToEnd ()
                    )
                failwith (sprintf "%O: %s" response.StatusCode contentString)

        if responseType = typedefof<Empty> then
            log.LogInformation ("HTTP response: {statusCode}", response.StatusCode)
            return (response.StatusCode, (new Empty ()) :> _)

        elif responseType = typedefof<string> then
            memoryStream.Seek (0L, System.IO.SeekOrigin.Begin) |> ignore
            let contentString =
                using (new System.IO.StreamReader (memoryStream, UTF8Encoding, false, 1024, true)) (fun streamReader ->
                    streamReader.ReadToEnd ()
                )
            log.LogInformation ("HTTP response: {statusCode} {content}", response.StatusCode, contentString)
            return (response.StatusCode, contentString :> _)

        else
            memoryStream.Seek (0L, System.IO.SeekOrigin.Begin) |> ignore
            let contentString =
                using (new System.IO.StreamReader (memoryStream, UTF8Encoding, false, 1024, true)) (fun streamReader ->
                    streamReader.ReadToEnd ()
                )
            log.LogInformation ("HTTP response: {statusCode} {content}", response.StatusCode, contentString)
            memoryStream.Seek (0L, System.IO.SeekOrigin.Begin) |> ignore

            use streamReader = new System.IO.StreamReader (memoryStream)
            return (response.StatusCode, serializer.Deserialize (streamReader, responseType))
    }

let Function
    (name: string)
    (log: Microsoft.Extensions.Logging.ILogger)
    (body: unit -> System.Threading.Tasks.Task<'a>)
    : System.Threading.Tasks.Task<'a> =
    FSharp.Control.Tasks.Builders.task {
        log.LogInformation ("{functionName} started at {startTime}", name, System.DateTime.UtcNow.ToString "o")

        try
            try
                return! body ()

            with ex ->
                log.LogError ("{message}: {stackTrace}", ex.Message, ex.StackTrace)
                raise ex
                return Unchecked.defaultof<_>
        finally
            log.LogInformation ("{functionName} ended at {endTime}", name, System.DateTime.UtcNow.ToString "o")
    }
