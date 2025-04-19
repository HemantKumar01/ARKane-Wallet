use crate::Error;
use bitcoin::hex::FromHex;
use std::io;
use std::io::Cursor;
use std::io::Read;
use std::io::Write;
use zkp::MusigPartialSignature;
use zkp::MusigPubNonce;

pub trait ToBytes {
    fn to_bytes(&self) -> Vec<u8>;
}

impl ToBytes for MusigPubNonce {
    fn to_bytes(&self) -> Vec<u8> {
        self.serialize().to_vec()
    }
}

impl ToBytes for MusigPartialSignature {
    fn to_bytes(&self) -> Vec<u8> {
        self.serialize().to_vec()
    }
}

pub trait FromCursor {
    fn from_cursor(cursor: &mut Cursor<&Vec<u8>>) -> Result<Self, Error>
    where
        Self: Sized;
}

impl FromCursor for MusigPubNonce {
    fn from_cursor(cursor: &mut Cursor<&Vec<u8>>) -> Result<Self, Error> {
        let mut buffer = [0u8; 66];
        cursor.read_exact(&mut buffer).map_err(Error::conversion)?;

        MusigPubNonce::from_slice(&buffer).map_err(Error::conversion)
    }
}

pub fn encode_tree<T>(tree: Vec<Vec<Option<T>>>) -> io::Result<Vec<u8>>
where
    T: ToBytes + std::fmt::Debug,
{
    let mut buf = Vec::new();

    let n_rows = tree.len() as u32;
    buf.write_all(&n_rows.to_le_bytes())?;

    for level in tree {
        let n_columns = level.len() as u32;
        buf.write_all(&n_columns.to_le_bytes())?;

        for elem in level {
            match elem {
                Some(elem) => {
                    buf.write_all(&[1u8])?;
                    buf.write_all(&elem.to_bytes())?;
                }
                None => {
                    buf.write_all(&[0u8])?;
                }
            }
        }
    }

    Ok(buf)
}

pub fn decode_tree<T>(serialized: String) -> Result<Vec<Vec<Option<T>>>, Error>
where
    T: FromCursor,
{
    let bytes = Vec::from_hex(&serialized).map_err(Error::conversion)?;
    let mut cursor = Cursor::new(&bytes);

    let mut n_rows = [0u8; 4];
    cursor.read_exact(&mut n_rows).map_err(Error::conversion)?;

    let n_rows = u32::from_le_bytes(n_rows);

    let mut matrix = Vec::with_capacity(n_rows as usize);

    for _ in 0..n_rows {
        let mut n_columns = [0u8; 4];
        cursor
            .read_exact(&mut n_columns)
            .map_err(Error::conversion)?;

        let n_columns = u32::from_le_bytes(n_columns);

        let mut row = Vec::with_capacity(n_columns as usize);

        for _ in 0..n_columns {
            let mut is_none = [0u8; 1];
            cursor.read_exact(&mut is_none).map_err(Error::conversion)?;

            let elem = match is_none.as_slice() {
                [0] => None,
                [1] => Some(T::from_cursor(&mut cursor)?),
                other => {
                    return Err(Error::conversion(format!(
                        "unexpected is_none value: {other:?}"
                    )))
                }
            };

            row.push(elem);
        }

        matrix.push(row);
    }

    Ok(matrix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::hex::DisplayHex;
    use bitcoin::hex::FromHex;
    use zkp::MusigPubNonce;
    use zkp::MusigSecNonce;

    #[test]
    fn nonce_tree_round_trip() {
        let a_bytes = Vec::from_hex("03a2ca7605303774152c9af458c9abdfa5636a8028e7bb91d4e2e6b69b60a7961e02e7d8f8d98e1b8452bec2b8132a49b97b8d3a5e8a71ce6d1b1b5a58d9263ac8dd").unwrap();
        let b_bytes = Vec::from_hex("021a9d01ba9ef321b512f1368ff426bb8e9a7edf4ae5f0e65691a08eef604acfc7026fc797f4f8a81af2f44aee6084a34227c16656eececa41d550fc1f0f6fe765fd").unwrap();

        let a = (
            MusigSecNonce::dangerous_from_bytes([1u8; 132]),
            MusigPubNonce::from_slice(&a_bytes).unwrap(),
        );
        let b = (
            MusigSecNonce::dangerous_from_bytes([2u8; 132]),
            MusigPubNonce::from_slice(&b_bytes).unwrap(),
        );

        let nonce_tree = vec![vec![Some(a.1)], vec![None, Some(b.1)]];

        let serialized = encode_tree(nonce_tree).unwrap().to_lower_hex_string();

        let deserialized = decode_tree(serialized).unwrap();

        let pub_nonce_tree = vec![
            vec![Some(MusigPubNonce::from_slice(&a_bytes).unwrap())],
            vec![None, Some(MusigPubNonce::from_slice(&b_bytes).unwrap())],
        ];

        assert_eq!(pub_nonce_tree, deserialized);
    }
}
