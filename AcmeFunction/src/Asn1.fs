module internal ArnavionDev.AzureFunctions.AcmeFunction.Asn1

// https://www.itu.int/rec/dologin_pub.asp?lang=e&id=T-REC-X.680-201508-I!!PDF-E&type=items
// https://www.itu.int/rec/dologin_pub.asp?lang=e&id=T-REC-X.690-201508-I!!PDF-E&type=items
// https://www.obj-sys.com/asn1tutorial/node12.html#tag

type internal Integer = System.Numerics.BigInteger

and internal ObjectIdentifier = string

and internal Sequence = Encodable list

and internal Encodable =
| ContextSpecific of byte * bool * Encodable
| Data of byte * byte list
| Integer of Integer
| Null
| ObjectIdentifier of ObjectIdentifier
| OctetString of byte array
| Sequence of Sequence

let rec private EncodedLength (bytes: byte list): byte list =
    let len = bytes.Length
    if len <= 0x0000007F then [len |> byte]
    elif len <= 0x000000FF then [0x81uy; len |> byte]
    elif len <= 0x0000FFFF then [0x82uy; len |> byte]
    else failwith (sprintf "data length %O is too large" len)

and internal Encode (value: Encodable): byte list =
    match value with
    | ContextSpecific (tag, explicit, value) ->
        let encodedValue = value |> Encode
        if explicit then
            Encodable.Data (0xA0uy ||| tag, encodedValue) |> Encode
        else
            match encodedValue with
            | _ :: rest -> ((0xA0uy ||| tag) :: (EncodedLength rest)) @ rest
            | [] -> failwith (sprintf "value %O encoded to zero bytes" value)

    | Data (tag, value) ->
        (tag :: (EncodedLength value)) @ value

    | Integer value ->
        let value = value.ToByteArray (false, true) |> List.ofArray
        Data (0x02uy, value) |> Encode

    | Null ->
        Data (0x05uy, []) |> Encode

    | ObjectIdentifier value ->
        let subIdentifiers = value.Split ('.') |> List.ofArray |> List.map System.UInt16.Parse
        let value =
            match subIdentifiers with
            | first :: second :: rest ->
                let first = first * 40us + second
                (first :: rest) |> List.collect (fun subIdentifier ->
                    if subIdentifier <= 0x007Fus then [subIdentifier |> byte]
                    elif subIdentifier <= 0x3FFFus then [(((subIdentifier &&& 0x3F80us) >>> 7) |> byte) ||| 0x80uy; (subIdentifier &&& 0x007Fus) |> byte]
                    else failwith (sprintf "object identifier %s has subidentifier %O that's too large" value subIdentifier)
                )

            | _ -> failwith (sprintf "malformed object identifier %s" value)

        Data (0x06uy, value) |> Encode

    | OctetString value ->
        let value = value |> List.ofArray
        Data (0x04uy, value) |> Encode

    | Sequence value ->
        let value = value |> List.collect Encode
        Data (0x30uy, value) |> Encode

let rec internal (|AsContextSpecific|_|) (bytes: byte list): (byte * bool * byte list * byte list) option =
    match bytes with
    | AsData (tag, value, rest) when tag &&& 0xC0uy = 0x80uy ->
        let actualTag = tag &&& 0x1Fuy
        let isConstructed = tag &&& 0x20uy <> 0x00uy
        Some (actualTag, isConstructed, value, rest)
    | _ -> None

and private (|AsData|_|) (bytes: byte list): (byte * byte list * byte list) option =
    match bytes with
    | id :: AsLength (len, rest) ->
        let len =
            match len with
            | Some len -> Some len
            | None ->
                let len = rest |> Seq.pairwise |> Seq.tryFindIndex (fun (b1, b2) -> b1 = 0x00uy && b2 = 0x00uy)
                match len with
                | Some len -> Some len
                | None -> None

        match len with
        | Some len when rest.Length >= len ->
            if len > 0 then
                Some (id, rest.[..(len - 1)], rest.[len..])
            else
                Some (id, [], rest)
        | _ -> None

    | _ -> None

and internal (|AsEnumerated|_|) (bytes: byte list): (Integer * byte list) option =
    match bytes with
    | AsData (0x0Auy, AsIntegerInner value, rest) -> Some (value, rest)
    | _ -> None

and internal (|AsIA5StringInner|_|) (bytes: byte list): string option =
    try
        let contents = bytes |> Array.ofList |> System.Text.Encoding.ASCII.GetString
        Some contents
    with
    | :? System.ArgumentException -> None

and internal (|AsInteger|_|) (bytes: byte list): (System.Numerics.BigInteger * byte list) option =
    match bytes with
    | AsData (0x02uy, AsIntegerInner value, rest) -> Some (value, rest)
    | _ -> None

and internal (|AsIntegerInner|_|) (bytes: byte list): System.Numerics.BigInteger option =
    let value = bytes |> Array.ofList
    let value = new System.ReadOnlySpan<byte> (value)
    let value = new System.Numerics.BigInteger (value, false, true)
    Some value

and private (|AsLength|_|) (bytes: byte list): (int option * byte list) option =
    match bytes with
    | b1 :: rest when b1 &&& 0x80uy = 0x00uy ->
        // definite form, short
        Some (Some (b1 |> int), rest)

    | 0x80uy :: rest ->
        // indefinite form
        Some (None, rest)

    | b1 :: rest when b1 &&& 0x80uy = 0x80uy && b1 <> 0xFFuy ->
        // definite form, long
        let numLengthBytes = (b1 &&& 0x7Fuy) |> int
        if rest.Length >= numLengthBytes then
            let length = rest.[..(numLengthBytes - 1)] |> List.fold (fun previous current -> (previous |> int) * 256 + (current |> int)) 0
            Some (Some length, rest.[numLengthBytes..])
        else
            None

    | _ -> None

and internal (|AsNull|_|) (bytes: byte list): byte list option =
    match bytes with
    | AsData (0x05uy, value, rest) when value.Length = 0 -> Some rest
    | _ -> None

and internal (|AsObjectIdentifier|_|) (bytes: byte list): (string * byte list) option =
    let rec accumulator (subIdentifiers: uint16 list) (bytes: byte list): (string * byte list) option =
        match bytes with
        | AsObjectSubIdentifier (value, rest) -> accumulator (value :: subIdentifiers) rest
        | rest ->
            match subIdentifiers |> List.rev with
            | first :: others ->
                let first, second =
                    if first < 40us then
                        0us, first
                    elif first < 80us then
                        1us, first - 40us
                    else
                        2us, first - 80us
                let subIdentifiers = first :: second :: others
                Some (subIdentifiers |> Seq.map (sprintf "%i") |> String.concat ".", rest)
            | [] -> None

    match bytes with
    | AsData (0x06uy, contents, rest) ->
        match accumulator [] contents with
        | Some (value, []) -> Some (value, rest)
        | _ -> None
    | _ -> None

and private (|AsObjectSubIdentifier|_|) (bytes: byte list): (uint16 * byte list) option =
    let rec accumulator (value: uint16) (bytes: byte list): (uint16 * byte list) option =
        match bytes with
        | b :: rest when b &&& 0x80uy = 0x00uy -> Some (value * 128us + (b |> uint16), rest)
        | b :: rest when b &&& 0x80uy = 0x80uy -> accumulator (value * 128us + ((b &&& 0x7Fuy) |> uint16)) rest
        | _ -> None

    accumulator 0us bytes

and internal (|AsOctetString|_|) (bytes: byte list): (byte array * byte list) option =
    match bytes with
    | AsData (0x04uy, value, rest) -> Some (value |> Array.ofList, rest)
    | _ -> None

and internal (|AsSequence|_|) (bytes: byte list): (byte list * byte list) option =
    match bytes with
    | AsData (0x30uy, contents, rest) -> Some (contents, rest)
    | _ -> None
