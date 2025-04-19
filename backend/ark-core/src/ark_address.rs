use crate::Error;
use bech32::Bech32m;
use bech32::Hrp;
use bitcoin::key::TweakedPublicKey;
use bitcoin::Network;
use bitcoin::ScriptBuf;
use bitcoin::XOnlyPublicKey;

#[derive(Debug, Clone, Copy)]
pub struct ArkAddress {
    hrp: Hrp,
    server: XOnlyPublicKey,
    vtxo_tap_key: TweakedPublicKey,
}

impl ArkAddress {
    pub fn to_p2tr_script_pubkey(&self) -> ScriptBuf {
        ScriptBuf::new_p2tr_tweaked(self.vtxo_tap_key)
    }
}

impl ArkAddress {
    pub fn new(network: Network, server: XOnlyPublicKey, vtxo_tap_key: TweakedPublicKey) -> Self {
        let hrp = match network {
            Network::Bitcoin => "ark",
            _ => "tark",
        };

        let hrp = Hrp::parse_unchecked(hrp);

        Self {
            hrp,
            server,
            vtxo_tap_key,
        }
    }

    pub fn encode(&self) -> String {
        let mut bytes = [0u8; 64];

        bytes[..32].copy_from_slice(&self.server.serialize());
        bytes[32..].copy_from_slice(&self.vtxo_tap_key.serialize());

        bech32::encode::<Bech32m>(self.hrp, bytes.as_slice()).expect("data can be encoded")
    }

    pub fn decode(value: &str) -> Result<Self, Error> {
        let (hrp, bytes) = bech32::decode(value).map_err(Error::address_format)?;

        let server = XOnlyPublicKey::from_slice(&bytes[..32]).map_err(Error::address_format)?;
        let vtxo_tap_key =
            XOnlyPublicKey::from_slice(&bytes[32..]).map_err(Error::address_format)?;

        // It is safe to call `dangerous_assume_tweaked` because we are treating the VTXO tap key as
        // finished product i.e. we are only going to use it as an address to send coins to.
        let vtxo_tap_key = TweakedPublicKey::dangerous_assume_tweaked(vtxo_tap_key);

        Ok(Self {
            hrp,
            server,
            vtxo_tap_key,
        })
    }
}

impl std::fmt::Display for ArkAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.encode())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::hex::DisplayHex;

    // Taken from https://github.com/ark-network/ark/blob/b536a9e65252573aaa48110ef5d0c90894eb550c/common/fixtures/encoding.json.
    #[test]
    fn roundtrip() {
        let address = "tark1x0lm8hhr2wc6n6lyemtyh9rz8rg2ftpkfun46aca56kjg3ws0tsztfpuanaquxc6faedvjk3tax0575y6perapg3e95654pk8r4fjecs5fyd2";

        let decoded = ArkAddress::decode(address).unwrap();

        let hrp = decoded.hrp.to_string();
        assert_eq!(hrp, "tark");

        let server = decoded.server.serialize().as_hex().to_string();
        assert_eq!(
            server,
            "33ffb3dee353b1a9ebe4ced64b946238d0a4ac364f275d771da6ad2445d07ae0"
        );

        let vtxo_tap_key = decoded.vtxo_tap_key.serialize().as_hex().to_string();
        assert_eq!(
            vtxo_tap_key,
            "25a43cecfa0e1b1a4f72d64ad15f4cfa7a84d0723e8511c969aa543638ea9967"
        );

        let encoded = decoded.encode();

        assert_eq!(encoded, address);
    }
}
