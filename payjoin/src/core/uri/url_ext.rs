use core::collections::BTreeMap;
use core::str::FromStr;

use bitcoin::bech32::Hrp;
use bitcoin::consensus::encode::Decodable;
use bitcoin::consensus::Encodable;
use url::Url;

use super::error::BadEndpointError;
use crate::hpke::HpkePublicKey;
use crate::ohttp::OhttpKeys;

/// Parse and set fragment parameters from `&pj=` URI parameter URLs
pub(crate) trait UrlExt {
    fn receiver_pubkey(&self) -> Result<HpkePublicKey, ParseReceiverPubkeyParamError>;
    fn set_receiver_pubkey(&mut self, exp: HpkePublicKey);
    fn ohttp(&self) -> Result<OhttpKeys, ParseOhttpKeysParamError>;
    fn set_ohttp(&mut self, ohttp: OhttpKeys);
    fn exp(&self) -> Result<core::time::SystemTime, ParseExpParamError>;
    fn set_exp(&mut self, exp: core::time::SystemTime);
}

impl UrlExt for Url {
    /// Retrieve the receiver's public key from the URL fragment
    fn receiver_pubkey(&self) -> Result<HpkePublicKey, ParseReceiverPubkeyParamError> {
        let value = get_param(self, "RK1")
            .map_err(ParseReceiverPubkeyParamError::InvalidFragment)?
            .ok_or(ParseReceiverPubkeyParamError::MissingPubkey)?;

        let (hrp, bytes) = crate::bech32::nochecksum::decode(value)
            .map_err(ParseReceiverPubkeyParamError::DecodeBech32)?;

        let rk_hrp: Hrp = Hrp::parse("RK").unwrap();
        if hrp != rk_hrp {
            return Err(ParseReceiverPubkeyParamError::InvalidHrp(hrp));
        }

        HpkePublicKey::from_compressed_bytes(&bytes[..])
            .map_err(ParseReceiverPubkeyParamError::InvalidPubkey)
    }

    /// Set the receiver's public key in the URL fragment
    fn set_receiver_pubkey(&mut self, pubkey: HpkePublicKey) {
        let rk_hrp: Hrp = Hrp::parse("RK").unwrap();

        set_param(
            self,
            &crate::bech32::nochecksum::encode(rk_hrp, &pubkey.to_compressed_bytes())
                .expect("encoding compressed pubkey bytes should never fail"),
        )
    }

    /// Retrieve the ohttp parameter from the URL fragment
    fn ohttp(&self) -> Result<OhttpKeys, ParseOhttpKeysParamError> {
        let value = get_param(self, "OH1")
            .map_err(ParseOhttpKeysParamError::InvalidFragment)?
            .ok_or(ParseOhttpKeysParamError::MissingOhttpKeys)?;
        OhttpKeys::from_str(value).map_err(ParseOhttpKeysParamError::InvalidOhttpKeys)
    }

    /// Set the ohttp parameter in the URL fragment
    fn set_ohttp(&mut self, ohttp: OhttpKeys) {
        set_param(self, &ohttp.to_string())
    }

    /// Retrieve the exp parameter from the URL fragment
    fn exp(&self) -> Result<core::time::SystemTime, ParseExpParamError> {
        let value = get_param(self, "EX1")
            .map_err(ParseExpParamError::InvalidFragment)?
            .ok_or(ParseExpParamError::MissingExp)?;

        let (hrp, bytes) =
            crate::bech32::nochecksum::decode(value).map_err(ParseExpParamError::DecodeBech32)?;

        let ex_hrp: Hrp = Hrp::parse("EX").unwrap();
        if hrp != ex_hrp {
            return Err(ParseExpParamError::InvalidHrp(hrp));
        }

        u32::consensus_decode(&mut &bytes[..])
            .map(|timestamp| {
                core::time::UNIX_EPOCH + core::time::Duration::from_secs(timestamp as u64)
            })
            .map_err(ParseExpParamError::InvalidExp)
    }

    /// Set the exp parameter in the URL fragment
    fn set_exp(&mut self, exp: core::time::SystemTime) {
        let t = match exp.duration_since(core::time::UNIX_EPOCH) {
            Ok(duration) => duration.as_secs().try_into().unwrap(), // TODO Result type instead of Option & unwrap
            Err(_) => 0u32,
        };

        let mut buf = [0u8; 4];
        t.consensus_encode(&mut &mut buf[..]).unwrap(); // TODO no unwrap

        let ex_hrp: Hrp = Hrp::parse("EX").unwrap();

        let exp_str = crate::bech32::nochecksum::encode(ex_hrp, &buf)
            .expect("encoding u32 timestamp should never fail");

        set_param(self, &exp_str)
    }
}

pub fn parse_with_fragment(endpoint: &str) -> Result<Url, BadEndpointError> {
    let url = Url::parse(endpoint).map_err(BadEndpointError::UrlParse)?;

    if let Some(fragment) = url.fragment() {
        if fragment.chars().any(|c| c.is_lowercase()) {
            return Err(BadEndpointError::LowercaseFragment);
        }
    };
    Ok(url)
}

#[derive(Debug)]
pub(crate) enum ParseFragmentError {
    InvalidChar(char),
    AmbiguousDelimiter,
}

impl core::error::Error for ParseFragmentError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        None
    }
}

impl core::fmt::Display for ParseFragmentError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        use ParseFragmentError::*;

        match &self {
            InvalidChar(c) => write!(f, "invalid character: {c} (must be uppercase)"),
            AmbiguousDelimiter => write!(f, "ambiguous fragment delimiter (both + and - found)"),
        }
    }
}

fn check_fragment_delimiter(fragment: &str) -> Result<char, ParseFragmentError> {
    // For backwards compatibility, also accept `+` as a
    // fragment parameter delimiter. This was previously
    // specified, but may be interpreted as ` ` by some
    // URI parsoing libraries. Therefore if `-` is missing,
    // assume the URI was generated following the older
    // version of the spec.

    let has_dash = fragment.contains('-');
    let has_plus = fragment.contains('+');

    // Even though fragment is a &str, it should be ascii so bytes() correspond
    // to chars(), except that it's easier to check that they are in range
    for c in fragment.bytes() {
        // These character ranges are more permissive than uppercase bech32, but
        // also more restrictive than bech32 in general since lowercase is not
        // allowed
        if !(b'0'..b'9' + 1).contains(&c)
            && !(b'A'..b'Z' + 1).contains(&c)
            && c != b'-'
            && c != b'+'
        {
            return Err(ParseFragmentError::InvalidChar(c.into()));
        }
    }

    match (has_dash, has_plus) {
        (true, true) => Err(ParseFragmentError::AmbiguousDelimiter),
        (false, true) => Ok('+'),
        _ => Ok('-'),
    }
}

fn get_param<'a>(url: &'a Url, prefix: &str) -> Result<Option<&'a str>, ParseFragmentError> {
    if let Some(fragment) = url.fragment() {
        let delim = check_fragment_delimiter(fragment)?;

        // The spec says these MUST be ordered lexicographically.
        // However, this was a late spec change, and only matters
        // for privacy reasons (fingerprinting implementations).
        // To maintain compatibility, we don't care about the order
        // of the parameters.
        for param in fragment.split(delim) {
            if param.starts_with(prefix) {
                return Ok(Some(param));
            }
        }
    }
    Ok(None)
}

/// Set a URL fragment parameter, inserting it or replacing it depending on
/// whether a parameter with the same bech32 HRP is already present.
///
/// Parameters are sorted lexicographically by prefix.
fn set_param(url: &mut Url, new_param: &str) {
    let fragment = url.fragment().unwrap_or("");
    let delim = check_fragment_delimiter(fragment)
        .expect("set_param must be called on a URL with a valid fragment");

    // In case of an invalid fragment parameter the following will still attempt
    // to retain the existing data
    let mut params = fragment
        .split(delim)
        .filter(|param| !param.is_empty())
        .map(|param| {
            let key = param.split('1').next().unwrap_or(param);
            (key, param)
        })
        .collect::<BTreeMap<&str, &str>>();

    // TODO: change param to Option(&str) to allow deletion?
    let key = new_param.split('1').next().unwrap_or(new_param);
    params.insert(key, new_param);

    if params.is_empty() {
        url.set_fragment(None)
    } else {
        // Can we avoid intermediate allocation of Vec, intersperse() exists but not in MSRV
        let fragment = params.values().copied().collect::<Vec<_>>().join("-");
        url.set_fragment(Some(&fragment));
    }
}

#[derive(Debug)]
pub(crate) enum ParseOhttpKeysParamError {
    MissingOhttpKeys,
    InvalidOhttpKeys(crate::ohttp::ParseOhttpKeysError),
    InvalidFragment(ParseFragmentError),
}

impl core::fmt::Display for ParseOhttpKeysParamError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        use ParseOhttpKeysParamError::*;

        match &self {
            MissingOhttpKeys => write!(f, "ohttp keys are missing"),
            InvalidOhttpKeys(o) => write!(f, "invalid ohttp keys: {o}"),
            InvalidFragment(e) => write!(f, "invalid URL fragment: {e}"),
        }
    }
}

#[derive(Debug)]
pub(crate) enum ParseExpParamError {
    MissingExp,
    InvalidHrp(bitcoin::bech32::Hrp),
    DecodeBech32(bitcoin::bech32::primitives::decode::CheckedHrpstringError),
    InvalidExp(bitcoin::consensus::encode::Error),
    InvalidFragment(ParseFragmentError),
}

impl core::fmt::Display for ParseExpParamError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        use ParseExpParamError::*;

        match &self {
            MissingExp => write!(f, "exp is missing"),
            InvalidHrp(h) => write!(f, "incorrect hrp for exp: {h}"),
            DecodeBech32(d) => write!(f, "exp is not valid bech32: {d}"),
            InvalidExp(i) => {
                write!(f, "exp param does not contain a bitcoin consensus encoded u32: {i}")
            }
            InvalidFragment(e) => write!(f, "invalid URL fragment: {e}"),
        }
    }
}

#[derive(Debug)]
pub(crate) enum ParseReceiverPubkeyParamError {
    MissingPubkey,
    InvalidHrp(bitcoin::bech32::Hrp),
    DecodeBech32(bitcoin::bech32::primitives::decode::CheckedHrpstringError),
    InvalidPubkey(crate::hpke::HpkeError),
    InvalidFragment(ParseFragmentError),
}

impl core::fmt::Display for ParseReceiverPubkeyParamError {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        use ParseReceiverPubkeyParamError::*;

        match &self {
            MissingPubkey => write!(f, "receiver public key is missing"),
            InvalidHrp(h) => write!(f, "incorrect hrp for receiver key: {h}"),
            DecodeBech32(e) => write!(f, "receiver public is not valid base64: {e}"),
            InvalidPubkey(e) => {
                write!(f, "receiver public key does not represent a valid pubkey: {e}")
            }
            InvalidFragment(e) => write!(f, "invalid URL fragment: {e}"),
        }
    }
}

impl core::error::Error for ParseReceiverPubkeyParamError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        use ParseReceiverPubkeyParamError::*;

        match &self {
            MissingPubkey => None,
            InvalidHrp(_) => None,
            DecodeBech32(error) => Some(error),
            InvalidPubkey(error) => Some(error),
            InvalidFragment(error) => Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use payjoin_test_utils::{BoxError, EXAMPLE_URL};

    use super::*;
    use crate::{Uri, UriExt};

    #[test]
    fn test_ohttp_get_set() {
        let mut url = EXAMPLE_URL.clone();

        let serialized = "OH1QYPM5JXYNS754Y4R45QWE336QFX6ZR8DQGVQCULVZTV20TFVEYDMFQC";
        let ohttp_keys = OhttpKeys::from_str(serialized).unwrap();
        url.set_ohttp(ohttp_keys.clone());

        assert_eq!(url.fragment(), Some(serialized));
        assert_eq!(
            url.ohttp().expect("Ohttp keys have been set but are missing on get"),
            ohttp_keys
        );
    }

    #[test]
    fn test_errors_when_parsing_ohttp() {
        let missing_ohttp_url = EXAMPLE_URL.clone();
        assert!(matches!(
            missing_ohttp_url.ohttp(),
            Err(ParseOhttpKeysParamError::MissingOhttpKeys)
        ));

        let invalid_ohttp_url =
            Url::parse("https://example.com?pj=https://test-payjoin-url#OH1invalid_bech_32")
                .unwrap();
        assert!(matches!(
            invalid_ohttp_url.ohttp(),
            Err(ParseOhttpKeysParamError::InvalidFragment(_))
        ));
    }

    #[test]
    fn test_exp_get_set() {
        let mut url = EXAMPLE_URL.clone();

        let exp_time =
            core::time::SystemTime::UNIX_EPOCH + core::time::Duration::from_secs(1720547781);
        url.set_exp(exp_time);
        assert_eq!(url.fragment(), Some("EX1C4UC6ES"));

        assert_eq!(url.exp().expect("Expiry has been set but is missing on get"), exp_time);
    }

    #[test]
    fn test_errors_when_parsing_exp() {
        let missing_exp_url = EXAMPLE_URL.clone();
        assert!(matches!(missing_exp_url.exp(), Err(ParseExpParamError::MissingExp)));

        let invalid_fragment_exp_url =
            Url::parse("http://example.com?pj=https://test-payjoin-url#EX1invalid_bech_32")
                .unwrap();
        assert!(matches!(
            invalid_fragment_exp_url.exp(),
            Err(ParseExpParamError::InvalidFragment(_))
        ));

        let invalid_bech32_exp_url =
            Url::parse("http://example.com?pj=https://test-payjoin-url#EX1INVALIDBECH32").unwrap();
        assert!(matches!(invalid_bech32_exp_url.exp(), Err(ParseExpParamError::DecodeBech32(_))));

        // Since the HRP is everything to the left of the right-most separator, the invalid url in
        // this test would have it's HRP being parsed as EX101 instead of the expected EX1
        let invalid_hrp_exp_url =
            Url::parse("http://example.com?pj=https://test-payjoin-url#EX1010").unwrap();
        assert!(matches!(invalid_hrp_exp_url.exp(), Err(ParseExpParamError::InvalidHrp(_))));

        // Not enough data to decode into a u32
        let invalid_timestamp_exp_url =
            Url::parse("http://example.com?pj=https://test-payjoin-url#EX10").unwrap();
        assert!(matches!(invalid_timestamp_exp_url.exp(), Err(ParseExpParamError::InvalidExp(_))));
    }

    #[test]
    fn test_errors_when_parsing_receiver_pubkey() {
        let missing_receiver_pubkey_url = EXAMPLE_URL.clone();
        assert!(matches!(
            missing_receiver_pubkey_url.receiver_pubkey(),
            Err(ParseReceiverPubkeyParamError::MissingPubkey)
        ));

        let invalid_fragment_receiver_pubkey_url =
            Url::parse("http://example.com?pj=https://test-payjoin-url#RK1invalid_bech_32")
                .unwrap();
        assert!(matches!(
            invalid_fragment_receiver_pubkey_url.receiver_pubkey(),
            Err(ParseReceiverPubkeyParamError::InvalidFragment(_))
        ));

        let invalid_bech32_receiver_pubkey_url =
            Url::parse("http://example.com?pj=https://test-payjoin-url#RK1INVALIDBECH32").unwrap();
        assert!(matches!(
            invalid_bech32_receiver_pubkey_url.receiver_pubkey(),
            Err(ParseReceiverPubkeyParamError::DecodeBech32(_))
        ));

        // Since the HRP is everything to the left of the right-most separator, the invalid url in
        // this test would have it's HRP being parsed as RK101 instead of the expected RK1
        let invalid_hrp_receiver_pubkey_url =
            Url::parse("http://example.com?pj=https://test-payjoin-url#RK101").unwrap();
        assert!(matches!(
            invalid_hrp_receiver_pubkey_url.receiver_pubkey(),
            Err(ParseReceiverPubkeyParamError::InvalidHrp(_))
        ));

        // Not enough data to decode into a u32
        let invalid_receiver_pubkey_url =
            Url::parse("http://example.com?pj=https://test-payjoin-url#RK10").unwrap();
        assert!(matches!(
            invalid_receiver_pubkey_url.receiver_pubkey(),
            Err(ParseReceiverPubkeyParamError::InvalidPubkey(_))
        ));
    }

    #[test]
    fn test_valid_v2_url_fragment_on_bip21() {
        let uri = "bitcoin:12c6DSiU4Rq3P4ZxziKxzrL5LmMBrzjrJX?amount=0.01\
                   &pjos=0&pj=HTTPS://EXAMPLE.COM/\
                   %23OH1QYPM5JXYNS754Y4R45QWE336QFX6ZR8DQGVQCULVZTV20TFVEYDMFQC";
        let pjuri = Uri::try_from(uri).unwrap().assume_checked().check_pj_supported().unwrap();
        assert!(pjuri.extras.endpoint().ohttp().is_ok());
        assert_eq!(format!("{pjuri}"), uri);

        let reordered = "bitcoin:12c6DSiU4Rq3P4ZxziKxzrL5LmMBrzjrJX?amount=0.01\
                   &pj=HTTPS://EXAMPLE.COM/\
                   %23OH1QYPM5JXYNS754Y4R45QWE336QFX6ZR8DQGVQCULVZTV20TFVEYDMFQC\
                   &pjos=0";
        let pjuri =
            Uri::try_from(reordered).unwrap().assume_checked().check_pj_supported().unwrap();
        assert!(pjuri.extras.endpoint().ohttp().is_ok());
        assert_eq!(format!("{pjuri}"), uri);
    }

    #[test]
    fn test_failed_url_fragment() -> Result<(), BoxError> {
        let expected_error = "LowercaseFragment";
        let uri = "bitcoin:12c6DSiU4Rq3P4ZxziKxzrL5LmMBrzjrJX?amount=0.01\
                   &pjos=0&pj=HTTPS://EXAMPLE.COM/\
                   %23oh1qypm5jxyns754y4r45qwe336qfx6zr8dqgvqculvztv20tfveydmfqc";
        assert!(Uri::try_from(uri).is_err(), "Expected url fragment failure, but it succeeded");
        if let Err(bitcoin_uri::de::Error::Extras(error)) = Uri::try_from(uri) {
            assert!(
                error.to_string().contains(expected_error),
                "Error should indicate '{expected_error}' but was: {error}"
            );
        }
        let uri = "bitcoin:12c6DSiU4Rq3P4ZxziKxzrL5LmMBrzjrJX?amount=0.01\
                   &pjos=0&pj=HTTPS://EXAMPLE.COM/\
                   %23OH1QYPM5JXYNS754Y4R45QWE336QFX6ZR8DQGVQCULVZTV20TFVEYDMFQc";
        assert!(Uri::try_from(uri).is_err(), "Expected url fragment failure, but it succeeded");
        if let Err(bitcoin_uri::de::Error::Extras(error)) = Uri::try_from(uri) {
            assert!(
                error.to_string().contains(expected_error),
                "Error should indicate '{expected_error}' but was: {error}"
            );
        }
        Ok(())
    }

    #[test]
    fn test_fragment_delimiter_backwards_compatibility() {
        // ensure + is still accepted as a delimiter
        let uri = "bitcoin:12c6DSiU4Rq3P4ZxziKxzrL5LmMBrzjrJX?amount=0.01\
                   &pjos=0&pj=HTTPS://EXAMPLE.COM/\
                   %23EX1C4UC6ES+OH1QYPM5JXYNS754Y4R45QWE336QFX6ZR8DQGVQCULVZTV20TFVEYDMFQC";
        let pjuri = Uri::try_from(uri).unwrap().assume_checked().check_pj_supported().unwrap();

        let mut endpoint = pjuri.extras.endpoint().clone();
        assert!(endpoint.ohttp().is_ok());
        assert!(endpoint.exp().is_ok());

        // Before setting the delimiter should be preserved
        assert_eq!(
            endpoint.fragment(),
            Some("EX1C4UC6ES+OH1QYPM5JXYNS754Y4R45QWE336QFX6ZR8DQGVQCULVZTV20TFVEYDMFQC")
        );

        // Upon setting any value, the delimiter should be normalized to `-`
        endpoint.set_exp(pjuri.extras.endpoint.exp().unwrap());
        assert_eq!(
            endpoint.fragment(),
            Some("EX1C4UC6ES-OH1QYPM5JXYNS754Y4R45QWE336QFX6ZR8DQGVQCULVZTV20TFVEYDMFQC")
        );
    }

    #[test]
    fn test_fragment_lexicographical_order() {
        let uri = "bitcoin:12c6DSiU4Rq3P4ZxziKxzrL5LmMBrzjrJX?amount=0.01\
                   &pjos=0&pj=HTTPS://EXAMPLE.COM/\
                   %23OH1QYPM5JXYNS754Y4R45QWE336QFX6ZR8DQGVQCULVZTV20TFVEYDMFQC-EX1C4UC6ES";
        let pjuri = Uri::try_from(uri).unwrap().assume_checked().check_pj_supported().unwrap();

        let mut endpoint = pjuri.extras.endpoint().clone();
        assert!(endpoint.ohttp().is_ok());
        assert!(endpoint.exp().is_ok());

        assert_eq!(
            endpoint.fragment(),
            Some("OH1QYPM5JXYNS754Y4R45QWE336QFX6ZR8DQGVQCULVZTV20TFVEYDMFQC-EX1C4UC6ES")
        );

        // Upon setting any value, the order should be normalized to lexicographical
        endpoint.set_exp(pjuri.extras.endpoint.exp().unwrap());
        assert_eq!(
            endpoint.fragment(),
            Some("EX1C4UC6ES-OH1QYPM5JXYNS754Y4R45QWE336QFX6ZR8DQGVQCULVZTV20TFVEYDMFQC")
        );
    }

    #[test]
    fn test_fragment_mixed_delimiter() {
        // mixing current and deprecated delimiters should fail
        let fragment = "23RK1QG2RH36X9ZWRK\
7UWCCQE0WD8T89XKK2W55KTK9UHSZLEG8Q2TGEGG-OH1QYP87E2AVMDKXDTU6R25WCPQ5ZUF02XHNPA65JMD8ZA2W4YRQN6UUWG+EX1XPK8Y6Q";
        assert!(matches!(
            check_fragment_delimiter(fragment),
            Err(ParseFragmentError::AmbiguousDelimiter)
        ));
    }
}
