module internal ArnavionDev.AzureFunctions.AcmeFunction.X509

// https://tools.ietf.org/html/rfc5280#section-4.1.1.2
type internal AlgorithmIdentifier = {
    Algorithm: Asn1.ObjectIdentifier
}

// https://tools.ietf.org/html/rfc5280#section-4.1
and internal CertificateSerialNumber = Asn1.Integer

// https://tools.ietf.org/html/rfc5280#section-4.1
and internal Extension = {
    ExtensionID: Asn1.ObjectIdentifier
    ExtensionValue: Asn1.OctetString
}

// https://tools.ietf.org/html/rfc5912#section-14
and private GeneralName =
| Uri of string

and internal Name = System.Security.Cryptography.X509Certificates.X500DistinguishedName

and internal Encodable =
| AlgorithmIdentifier of AlgorithmIdentifier
| CertificateSerialNumber of CertificateSerialNumber
| Extension of Extension

let rec Encode (value: Encodable): Asn1.Encodable =
    match value with
    | AlgorithmIdentifier value ->
        Asn1.Encodable.Sequence [
            value.Algorithm |> Asn1.Encodable.ObjectIdentifier;
        ]

    | CertificateSerialNumber value ->
        value |> Asn1.Encodable.Integer

    | Extension value ->
        Asn1.Encodable.Sequence [
            value.ExtensionID |> Asn1.Encodable.ObjectIdentifier;
            value.ExtensionValue |> Asn1.Encodable.OctetString;
        ]

let rec private (|AsAccessDescription|_|) (bytes: byte list): (string * GeneralName * byte list) option =
    match bytes with
    | Asn1.AsSequence (Asn1.AsObjectIdentifier (oid, AsGeneralName (value, [])), rest) -> Some (oid, value, rest)
    | _ -> None

and internal (|AsAlgorithmIdentifier|_|) (bytes: byte list): (AlgorithmIdentifier * byte list) option =
    match bytes with
    | Asn1.AsSequence (Asn1.AsObjectIdentifier (value, []), rest) ->
        Some ({
            Algorithm = value
        }, rest)
    | _ -> None

and private (|AsGeneralName|_|) (bytes: byte list): (GeneralName * byte list) option =
    match bytes with
    | Asn1.AsContextSpecific (0x06uy, false, Asn1.AsIA5StringInner value, rest) -> Some (Uri value, rest)
    | _ -> None

and internal (|AsName|_|) (value: Asn1.Type): Name option =
    let bytes = value |> Asn1.Encode
    try
        let value = new System.Security.Cryptography.X509Certificates.X500DistinguishedName (bytes)
        Some value
    with
    | _ -> None

// https://tools.ietf.org/html/rfc5280#section-4.2.2.1
let internal GetOcspUris (bytes: byte array): string list =
    let rec accumulator (uris: string list) (bytes: byte list): string list =
        match bytes with
        | AsAccessDescription ("1.3.6.1.5.5.7.48.1", GeneralName.Uri value, rest) -> accumulator (value :: uris) rest
        | AsAccessDescription(_, _, rest) -> accumulator uris rest
        | _ -> uris |> List.rev

    let bytes = bytes |> List.ofArray
    match bytes with
    | Asn1.AsSequence (content, []) -> accumulator [] content
    | _ -> []
