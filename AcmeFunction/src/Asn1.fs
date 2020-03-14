module internal ArnavionDev.AzureFunctions.AcmeFunction.Asn1

// https://www.itu.int/rec/dologin_pub.asp?lang=e&id=T-REC-X.680-201508-I!!PDF-E&type=items
// https://www.itu.int/rec/dologin_pub.asp?lang=e&id=T-REC-X.690-201508-I!!PDF-E&type=items
// https://www.obj-sys.com/asn1tutorial/node12.html#tag


type internal BitString = System.Collections.BitArray

and internal GeneralizedTime = System.DateTime

and internal Integer = System.Numerics.BigInteger

and internal ObjectIdentifier = string

and internal OctetString = byte array

and internal Sequence = Type list

and Type =
| BitString of BitString
| ContextSpecificExplicit of byte * Type
| ContextSpecificImplicit of byte * System.ReadOnlyMemory<byte>
| Enumerated of Integer
| GeneralizedTime of GeneralizedTime
| Integer of Integer
| Null
| ObjectIdentifier of ObjectIdentifier
| OctetString of OctetString
| Sequence of Sequence

let internal Decode (bytes: System.ReadOnlyMemory<byte>): Type =
    let rec (|AsData|_|) (bytes: System.ReadOnlyMemory<byte>): (byte * System.ReadOnlyMemory<byte> * System.ReadOnlyMemory<byte>) option =
        if bytes.IsEmpty then
            None
        else
            let id = bytes.Span.[0]
            match bytes.Slice (1) with
            | AsLength (len, rest) ->
                let len =
                    match len with
                    | Some len -> Some len
                    | None ->
                        let len =
                            rest
                            |> System.Runtime.InteropServices.MemoryMarshal.ToEnumerable
                            |> Seq.pairwise
                            |> Seq.tryFindIndex (fun (b1, b2) -> b1 = 0x00uy && b2 = 0x00uy)
                        match len with
                        | Some len -> Some len
                        | None -> None

                match len with
                | Some len when rest.Length >= len ->
                    Some (id, rest.Slice (0, len), rest.Slice (len))

                | _ -> None

            | _ -> None

    and (|AsLength|_|) (bytes: System.ReadOnlyMemory<byte>): (int option * System.ReadOnlyMemory<byte>) option =
        if bytes.IsEmpty then
            None
        else
            let b1 = bytes.Span.[0]
            let rest = bytes.Slice (1)

            if b1 &&& 0x80uy = 0x00uy then
                // definite form, short
                Some (Some (b1 |> int), rest)
            elif b1 = 0x80uy then
                // indefinite form
                Some (None, rest)
            elif b1 = 0xFFuy then
                // invalid
                None
            else
                // definite form, long
                let numLengthBytes = (b1 &&& 0x7Fuy) |> int
                if rest.Length >= numLengthBytes then
                    let len =
                        rest.Slice (0, numLengthBytes)
                        |> System.Runtime.InteropServices.MemoryMarshal.ToEnumerable
                        |> Seq.fold (fun previous current -> (previous |> int) * 256 + (current |> int)) 0
                    Some (Some len, rest.Slice (numLengthBytes))
                else
                    None

    and (|AsIA5StringInner|_|) (bytes: System.ReadOnlyMemory<byte>): string option =
        try
            let contents = bytes.ToArray () |> System.Text.Encoding.ASCII.GetString
            Some contents
        with
        | :? System.ArgumentException -> None

    and (|AsIntegerInner|_|) (bytes: System.ReadOnlyMemory<byte>): System.Numerics.BigInteger option =
        let value = bytes.ToArray ()
        let value = new System.ReadOnlySpan<byte> (value)
        let value = new System.Numerics.BigInteger (value, false, true)
        Some value

    let rec InnerDecode (bytes: System.ReadOnlyMemory<byte>): Type * System.ReadOnlyMemory<byte> =
        match bytes with
        | AsData (0x02uy, AsIntegerInner value, rest) ->
            Type.Integer value, rest

        | AsData (0x03uy, bytes, rest) ->
            let bitString = new System.Collections.BitArray (bytes.ToArray ())
            bitString.Length <- bytes.Length
            Type.BitString bitString, rest

        | AsData (0x04uy, bytes, rest) ->
            let value = bytes.ToArray ()
            Type.OctetString value, rest

        | AsData (0x05uy, bytes, rest) when bytes.Length = 0 ->
            Type.Null, rest

        | AsData (0x06uy, bytes, rest) ->
            let (|AsObjectSubIdentifier|_|) (bytes: System.ReadOnlyMemory<byte>): (uint16 * System.ReadOnlyMemory<byte>) option =
                let rec accumulator (value: uint16) (bytes: System.ReadOnlyMemory<byte>): (uint16 * System.ReadOnlyMemory<byte>) option =
                    if bytes.IsEmpty then
                        None
                    else
                        let b = bytes.Span.[0]
                        let rest = bytes.Slice (1)
                        if b &&& 0x80uy = 0x00uy then
                            Some (value * 128us + (b |> uint16), rest)
                        else
                            accumulator (value * 128us + ((b &&& 0x7Fuy) |> uint16)) rest

                accumulator 0us bytes

            let rec objectIdentifierAccumulator (subIdentifiers: uint16 list) (bytes: System.ReadOnlyMemory<byte>): (string * System.ReadOnlyMemory<byte>) option =
                match bytes with
                | AsObjectSubIdentifier (value, rest) -> objectIdentifierAccumulator (value :: subIdentifiers) rest
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

            match objectIdentifierAccumulator [] bytes with
            | Some (value, rest) when rest.IsEmpty -> Type.ObjectIdentifier value, rest
            | _ -> failwith (sprintf "could not parse ObjectIdentifier from %A" (bytes.ToArray ()))

        | AsData (0x0Auy, AsIntegerInner value, rest) ->
            Type.Enumerated value, rest

        | AsData (0x18uy, bytes, rest) ->
            let value = bytes.ToArray () |> System.Text.Encoding.ASCII.GetString
            let fractionalFormat =
                match value.Length with
                | 15 -> ""
                | len when len >= 16 && len <= 23 -> "." + new System.String ('f', len - 16)
                | _ -> failwith (sprintf "could not parse DateTime from %A" (bytes.ToArray ()))
            let value =
                System.DateTime.ParseExact (
                    value,
                    "yyyyMMddHHmmss" + fractionalFormat + "Z",
                    System.Globalization.CultureInfo.InvariantCulture
                )
            Type.GeneralizedTime value, rest

        | AsData (0x30uy, contents, rest) ->
            let rec accumulator (elements: Type list) (bytes: System.ReadOnlyMemory<byte>): Type list =
                let value, rest = bytes |> InnerDecode
                let elements = value :: elements
                if rest.IsEmpty then
                    elements |> List.rev
                else
                    accumulator elements rest

            let elements = accumulator [] bytes
            Type.Sequence elements, rest

        | AsData (tag, bytes, rest) when tag &&& 0xC0uy = 0x80uy ->
            let actualTag = tag &&& 0x1Fuy
            let isConstructed = tag &&& 0x20uy <> 0x00uy
            Type.ContextSpecific (actualTag, isConstructed, bytes), rest

        | _ -> failwith (sprintf "could not parse ASN.1 value from %A" (bytes.ToArray ()))

    let value = bytes |> InnerDecode
    match value with
    | value, _ -> value

let rec internal Encode (value: Type): byte list =
    let rec EncodeData (tag: byte) (value: byte list): byte list =
        (tag :: (EncodedLength value)) @ value

    and EncodedLength (bytes: byte list): byte list =
        let len = bytes.Length
        if len <= 0x0000007F then [len |> byte]
        elif len <= 0x000000FF then [0x81uy; len |> byte]
        elif len <= 0x0000FFFF then [0x82uy; len |> byte]
        else failwith (sprintf "data length %O is too large" len)

    match value with
    | ContextSpecific (tag, explicit, value) ->
        let encodedValue = value |> Encode
        if explicit then
            EncodeData (0xA0uy ||| tag, encodedValue) |> Encode
        else
            match encodedValue with
            | _ :: rest -> ((0xA0uy ||| tag) :: (EncodedLength rest)) @ rest
            | [] -> failwith (sprintf "value %O encoded to zero bytes" value)

    | Data (tag, value) ->
        (tag :: (EncodedLength value)) @ value

    // | GeneralizedTime value ->
    //     if value.Kind <> System.DateTimeKind.Utc then
    //         failwith (sprintf "GeneralizedTime requires Utc DateTime but was given %O" value)
    //     let value = value.ToString "yyyyMMddHHmmss.FFFFFFF"
    //     let value = value.TrimEnd '.'
    //     let value = value |> System.Text.Encoding.ASCII.GetBytes |> List.ofArray
    //     EncodeData (0x24uy, value) |> Encode

    | Integer value ->
        let value = value.ToByteArray (false, true) |> List.ofArray
        EncodeData (0x02uy, value) |> Encode

    | Null ->
        EncodeData (0x05uy, []) |> Encode

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

        EncodeData (0x06uy, value) |> Encode

    | OctetString value ->
        let value = value |> List.ofArray
        EncodeData (0x04uy, value) |> Encode

    | Sequence value ->
        let value = value |> List.collect Encode
        EncodeData (0x30uy, value) |> Encode

    | value ->
        failwith (sprintf "unimplemented Asn1.Encode for %O" value)
