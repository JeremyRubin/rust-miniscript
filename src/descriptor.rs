// Script Descriptor Language
// Written in 2018 by
//     Andrew Poelstra <apoelstra@wpsoftware.net>
//
// To the extent possible under law, the author(s) have dedicated all
// copyright and related and neighboring rights to this software to
// the public domain worldwide. This software is distributed without
// any warranty.
//
// You should have received a copy of the CC0 Public Domain Dedication
// along with this software.
// If not, see <http://creativecommons.org/publicdomain/zero/1.0/>.
//

//! # Script Descriptors
//!
//! Tools for representing Bitcoin scriptpubkeys as abstract spending policies, known
//! as "script descriptors".
//!
//! The format represents EC public keys abstractly to allow wallets to replace these with
//! BIP32 paths, pay-to-contract instructions, etc.
//!

use std::fmt;
use std::str::{self, FromStr};

use secp256k1;

use bitcoin::util::hash::Sha256dHash; // TODO needs to be sha256, not sha256d

use Error;

/// Script descriptor
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Descriptor<P> {
    /// A public key which must sign to satisfy the descriptor
    Key(P),
    /// A public key which must sign to satisfy the descriptor (pay-to-pubkey-hash form)
    KeyHash(P),
    /// A set of keys, signatures must be provided for `k` of them
    Multi(usize, Vec<P>),
    /// A SHA256 whose preimage must be provided to satisfy the descriptor
    Hash(Sha256dHash),
    /// A locktime restriction
    Time(u32),
    /// A set of descriptors, satisfactions must be provided for `k` of them
    Threshold(usize, Vec<Descriptor<P>>),
    /// A list of descriptors, all of which must be satisfied
    And(Box<Descriptor<P>>, Box<Descriptor<P>>),
    /// A pair of descriptors, one of which must be satisfied
    Or(Box<Descriptor<P>>, Box<Descriptor<P>>),
    /// Same as `Or`, but the second option is assumed to never be taken for costing purposes
    AsymmetricOr(Box<Descriptor<P>>, Box<Descriptor<P>>),
    /// Pay-to-Witness-PubKey-Hash
    Wpkh(P),
    /// Pay-to-ScriptHash
    Sh(Box<Descriptor<P>>),
    /// Pay-to-Witness-ScriptHash
    Wsh(Box<Descriptor<P>>),
}

impl<P> Descriptor<P> {
    /// Convert a descriptor using abstract keys to one using specific keys
    pub fn instantiate<F, E>(&self, instantiate_fn: &F) -> Result<Descriptor<secp256k1::PublicKey>, E>
        where F: Fn(&P) -> Result<secp256k1::PublicKey, E> {

        match *self {
            Descriptor::Key(ref pk) => {
                instantiate_fn(pk).map(Descriptor::Key)
            }
            Descriptor::KeyHash(ref pk) => {
                instantiate_fn(pk).map(Descriptor::KeyHash)
            }
            Descriptor::Multi(k, ref keys) => {
                let mut new_keys = Vec::with_capacity(keys.len());
                for key in keys {
                    let secp_pk = instantiate_fn(key)?;
                    new_keys.push(secp_pk);
                }
                Ok(Descriptor::Multi(k, new_keys))
            }
            Descriptor::Threshold(k, ref subs) => {
                let mut new_subs = Vec::with_capacity(subs.len());
                for sub in subs {
                    new_subs.push(sub.instantiate(instantiate_fn)?);
                }
                Ok(Descriptor::Threshold(k, new_subs))
            }
            Descriptor::Hash(hash) => Ok(Descriptor::Hash(hash)),
            Descriptor::And(ref left, ref right) => {
                Ok(Descriptor::And(
                    Box::new(left.instantiate(instantiate_fn)?),
                    Box::new(right.instantiate(instantiate_fn)?)
                ))
            }
            Descriptor::Or(ref left, ref right) => {
                Ok(Descriptor::Or(
                    Box::new(left.instantiate(instantiate_fn)?),
                    Box::new(right.instantiate(instantiate_fn)?)
                ))
            }
            Descriptor::AsymmetricOr(ref left, ref right) => {
                Ok(Descriptor::AsymmetricOr(
                    Box::new(left.instantiate(instantiate_fn)?),
                    Box::new(right.instantiate(instantiate_fn)?)
                ))
            }
            Descriptor::Time(n) => Ok(Descriptor::Time(n)),
            Descriptor::Wpkh(ref pk) => {
                instantiate_fn(pk).map(Descriptor::Wpkh)
            }
            Descriptor::Sh(ref desc) => {
                Ok(Descriptor::Sh(Box::new(desc.instantiate(instantiate_fn)?)))
            }
            Descriptor::Wsh(ref desc) => {
                Ok(Descriptor::Wsh(Box::new(desc.instantiate(instantiate_fn)?)))
            }
        }
    }
}

impl<P: FromStr> Descriptor<P>
    where P::Err: ToString + fmt::Debug
{
    fn from_tree(top: &FunctionTree) -> Result<Descriptor<P>, Error> {
        match (top.name, top.args.len() as u32) {
            ("pk", 1) => {
                let pk = &top.args[0];
                if pk.args.is_empty() {
                    match P::from_str(pk.name) {
                        Ok(pk) => Ok(Descriptor::Key(pk)),
                        Err(e) => Err(Error::Unexpected(e.to_string())),
                    }
                } else {
                    Err(errorize(pk.args[0].name))
                }
            }
            ("pkh", 1) => {
                let pk = &top.args[0];
                if pk.args.is_empty() {
                    match P::from_str(pk.name) {
                        Ok(pk) => Ok(Descriptor::KeyHash(pk)),
                        Err(e) => Err(Error::Unexpected(e.to_string())),
                    }
                } else {
                    Err(errorize(pk.args[0].name))
                }
            }
            ("multi", nkeys) => {
                for arg in &top.args {
                    if !arg.args.is_empty() {
                        return Err(errorize(arg.args[0].name));
                    }
                }

// TODO ** special case empty multis
                let thresh = match parse_num(top.args[0].name) {
                    Ok(n) => n,
                    Err(_) => {
                        return Ok(Descriptor::Multi(2, vec![
                            P::from_str("").unwrap(),
                            P::from_str("").unwrap(),
                            P::from_str("").unwrap(),
                        ]));
                    }
                };
// end TODO ** special case empty multis
                if thresh >= nkeys {
                    return Err(errorize("higher threshold than there were keys in multi"));
                }

                let mut keys = Vec::with_capacity(top.args.len() - 1);
                for arg in &top.args[1..] {
                    match P::from_str(arg.name) {
                        Ok(pk) => keys.push(pk),
                        Err(e) => return Err(Error::Unexpected(e.to_string())),
                    }
                }
                Ok(Descriptor::Multi(thresh as usize, keys))
            }
            ("hash", 1) => {
// TODO ** special case empty strings
if top.args[0].args.is_empty() && top.args[0].name == "" {
    return Ok(Descriptor::Hash(Sha256dHash::from_data(&[0;32][..])));
}
// TODO ** special case empty strings
                let hash_t = &top.args[0];
                if hash_t.args.is_empty() {
                    if let Ok(hash) = Sha256dHash::from_hex(hash_t.args[0].name) {
                        Ok(Descriptor::Hash(hash))
                    } else {
                        Err(errorize(hash_t.args[0].name))
                    }
                } else {
                    Err(errorize(hash_t.args[0].name))
                }
            }
            ("time", 1) => {
// TODO ** special case empty strings
if top.args[0].args.is_empty() && top.args[0].name == "" {
    return Ok(Descriptor::Time(0x10000000))
}
// TODO ** special case empty strings
                let time_t = &top.args[0];
                if time_t.args.is_empty() {
                    Ok(Descriptor::Time(parse_num(time_t.args[0].name)?))
                } else {
                    Err(errorize(time_t.args[0].name))
                }
            }
            ("thres", nsubs) => {
                if !top.args[0].args.is_empty() {
                    return Err(errorize(top.args[0].args[0].name));
                }

                let thresh = parse_num(top.args[0].name)?;
                if thresh >= nsubs {
                    return Err(errorize(top.args[0].name));
                }

                let mut subs = Vec::with_capacity(top.args.len() - 1);
                for arg in &top.args[1..] {
                    subs.push(Descriptor::from_tree(arg)?);
                }
                Ok(Descriptor::Threshold(thresh as usize, subs))
            }
            ("and", 2) => {
                Ok(Descriptor::And(
                    Box::new(Descriptor::from_tree(&top.args[0])?),
                    Box::new(Descriptor::from_tree(&top.args[1])?),
                ))
            }
            ("or", 2) => {
                Ok(Descriptor::Or(
                    Box::new(Descriptor::from_tree(&top.args[0])?),
                    Box::new(Descriptor::from_tree(&top.args[1])?),
                ))
            }
            ("aor", 2) => {
                Ok(Descriptor::AsymmetricOr(
                    Box::new(Descriptor::from_tree(&top.args[0])?),
                    Box::new(Descriptor::from_tree(&top.args[1])?),
                ))
            }
            ("wpkh", 1) => {
                let pk = &top.args[0];
                if pk.args.is_empty() {
                    match P::from_str(pk.name) {
                        Ok(pk) => Ok(Descriptor::Wpkh(pk)),
                        Err(e) => Err(Error::Unexpected(e.to_string())),
                    }
                } else {
                    Err(errorize(pk.args[0].name))
                }
            }
            ("sh", 1) => {
                let sub = Descriptor::from_tree(&top.args[0])?;
                Ok(Descriptor::Sh(Box::new(sub)))
            }
            ("wsh", 1) => {
                let sub = Descriptor::from_tree(&top.args[0])?;
                Ok(Descriptor::Wsh(Box::new(sub)))
            }
            _ => Err(errorize(top.name))
        }
    }
}

fn errorize(s: &str) -> Error {
    Error::Unexpected(s.to_owned())
}

fn parse_num(s: &str) -> Result<u32, Error> {
    u32::from_str(s).map_err(|_| errorize(s))
}

impl<P: FromStr> FromStr for Descriptor<P>
    where P::Err: ToString + fmt::Debug
{
    type Err = Error;

    fn from_str(s: &str) -> Result<Descriptor<P>, Error> {
        for ch in s.as_bytes() {
            if *ch < 20 || *ch > 127 {
                return Err(Error::Unprintable(*ch));
            }
        }

        let (top, rem) = FunctionTree::from_slice(s)?;
        if !rem.is_empty() {
            return Err(errorize(rem));
        }
        Descriptor::from_tree(&top)
    }
}

impl <P: fmt::Display> fmt::Display for Descriptor<P> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Descriptor::Key(ref p) => {
                f.write_str("pk(")?;
                p.fmt(f)?;
            }
            Descriptor::KeyHash(ref p) => {
                f.write_str("pkh(")?;
                p.fmt(f)?;
            }
            Descriptor::Multi(k, ref keys) => {
                write!(f, "multi({}", k)?;
                for key in keys {
                    key.fmt(f)?;
                    f.write_str(",")?;
                }
            }
            Descriptor::Hash(hash) => {
                write!(f, "hash({}", hash)?;
            }
            Descriptor::Time(n) => {
                write!(f, "time({}", n)?;
            }
            Descriptor::Threshold(k, ref descs) => {
                write!(f, "multi({}", k)?;
                for desc in descs {
                    write!(f, "{},", desc)?;
                }
            }
            Descriptor::And(ref left, ref right) => {
                write!(f, "and({}, {}", left, right)?;
            }
            Descriptor::Or(ref left, ref right) => {
                write!(f, "or({}, {}", left, right)?;
            }
            Descriptor::AsymmetricOr(ref left, ref right) => {
                write!(f, "aor({}, {}", left, right)?;
            }
            Descriptor::Wpkh(ref p) => {
                f.write_str("wpkh(")?;
                p.fmt(f)?;
            }
            Descriptor::Sh(ref desc) => {
                write!(f, "sh({}", desc)?;
            }
            Descriptor::Wsh(ref desc) => {
                write!(f, "wsh({}", desc)?;
            }
        }
        f.write_str(")")
    }
}

#[derive(Debug)]
struct FunctionTree<'a> {
    name: &'a str,
    args: Vec<FunctionTree<'a>>,
}

impl<'a> FunctionTree<'a> {
    fn from_slice(mut sl: &'a str) -> Result<(FunctionTree<'a>, &'a str), Error> {
        enum Found { Nothing, Lparen(usize), Comma(usize), Rparen(usize) }

        let mut found = Found::Nothing;
        for (n, ch) in sl.chars().enumerate() {
            match ch {
                '(' => { found = Found::Lparen(n); break; }
                ',' => { found = Found::Comma(n); break; }
                ')' => { found = Found::Rparen(n); break; }
                _ => {}
            }
        }

        match found {
            // Unexpected EOF
            Found::Nothing => Err(Error::ExpectedChar(')')),
            // Terminal
            Found::Comma(n) | Found::Rparen(n) => {
                Ok((
                    FunctionTree {
                        name: &sl[..n],
                        args: vec![],
                    },
                    &sl[n..],
                ))
            }
            // Function call
            Found::Lparen(n) => {
                let mut ret = FunctionTree {
                    name: &sl[..n],
                    args: vec![],
                };

                sl = &sl[n + 1..];
                loop {
                    let (arg, new_sl) = FunctionTree::from_slice(sl)?;
                    ret.args.push(arg);

                    if new_sl.is_empty() {
                        return Err(Error::ExpectedChar(')'));
                    }

                    sl = &new_sl[1..];
                    match new_sl.as_bytes()[0] {
                        b',' => {},
                        b')' => break,
                        _ => return Err(Error::ExpectedChar(','))
                    }
                }
                Ok((ret, sl))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use secp256k1;
    use std::collections::HashMap;
    use std::str::FromStr;

    use bitcoin::blockdata::opcodes;
    use bitcoin::blockdata::script::{self, Script};
    use bitcoin::blockdata::transaction::SigHashType;
    use Descriptor;
    use ParseTree;

    fn pubkeys_and_a_sig(n: usize) -> (Vec<secp256k1::PublicKey>, secp256k1::Signature) {
        let mut ret = Vec::with_capacity(n);
        let secp = secp256k1::Secp256k1::new();
        let mut sk = [0; 32];
        for i in 1..n+1 {
            sk[0] = i as u8;
            sk[1] = (i >> 8) as u8;
            sk[2] = (i >> 16) as u8;

            let pk = secp256k1::PublicKey::from_secret_key(
                &secp,
                &secp256k1::SecretKey::from_slice(&secp, &sk[..]).expect("secret key"),
            );
            ret.push(pk);
        }
        let sig = secp.sign(
            &secp256k1::Message::from_slice(&sk[..]).expect("secret key"),
            &secp256k1::SecretKey::from_slice(&secp, &sk[..]).expect("secret key"),
        );
        (ret, sig)
    }

    #[test]
    fn compile() {
        let (keys, sig) = pubkeys_and_a_sig(10);
        let desc: Descriptor<secp256k1::PublicKey> = Descriptor::Time(100);
        let pt = ParseTree::compile(&desc);
        assert_eq!(pt.serialize(), Script::from(vec![0x01, 0x64, 0xb2]));

        let desc = Descriptor::Key(keys[0].clone());
        let pt = ParseTree::compile(&desc);
        assert_eq!(
            pt.serialize(),
            script::Builder::new()
                .push_slice(&keys[0].serialize()[..])
                .push_opcode(opcodes::All::OP_CHECKSIG)
                .into_script()
        );

        // CSV reordering trick
        let desc = Descriptor::And(
            // nb the compiler will reorder this because it can avoid the DROP if it ends with the CSV
            Box::new(Descriptor::Time(10000)),
            Box::new(Descriptor::Multi(2, keys[5..8].to_owned())),
        );
        let pt = ParseTree::compile(&desc);
        assert_eq!(
            pt.serialize(),
            script::Builder::new()
                .push_opcode(opcodes::All::OP_PUSHNUM_2)
                .push_slice(&keys[5].serialize()[..])
                .push_slice(&keys[6].serialize()[..])
                .push_slice(&keys[7].serialize()[..])
                .push_opcode(opcodes::All::OP_PUSHNUM_3)
                .push_opcode(opcodes::All::OP_CHECKMULTISIGVERIFY)
                .push_int(10000)
                .push_opcode(opcodes::OP_CSV)
                .into_script()
        );

        // Liquid policy
        let desc = Descriptor::AsymmetricOr(
            Box::new(Descriptor::Multi(3, keys[0..5].to_owned())),
            Box::new(Descriptor::And(
                Box::new(Descriptor::Time(10000)),
                Box::new(Descriptor::Multi(2, keys[5..8].to_owned())),
            )),
        );
        let pt = ParseTree::compile(&desc);
        assert_eq!(
            pt.serialize(),
            script::Builder::new()
                .push_opcode(opcodes::All::OP_PUSHNUM_3)
                .push_slice(&keys[0].serialize()[..])
                .push_slice(&keys[1].serialize()[..])
                .push_slice(&keys[2].serialize()[..])
                .push_slice(&keys[3].serialize()[..])
                .push_slice(&keys[4].serialize()[..])
                .push_opcode(opcodes::All::OP_PUSHNUM_5)
                .push_opcode(opcodes::All::OP_CHECKMULTISIG)
                .push_opcode(opcodes::All::OP_IFDUP)
                .push_opcode(opcodes::All::OP_NOTIF)
                    .push_opcode(opcodes::All::OP_PUSHNUM_2)
                    .push_slice(&keys[5].serialize()[..])
                    .push_slice(&keys[6].serialize()[..])
                    .push_slice(&keys[7].serialize()[..])
                    .push_opcode(opcodes::All::OP_PUSHNUM_3)
                    .push_opcode(opcodes::All::OP_CHECKMULTISIGVERIFY)
                    .push_int(10000)
                    .push_opcode(opcodes::OP_CSV)
                .push_opcode(opcodes::All::OP_ENDIF)
                .into_script()
        );

        assert_eq!(
            &pt.required_keys()[..],
            &keys[0..8]
        );

        let mut sigvec = sig.serialize_der(&secp256k1::Secp256k1::without_caps());
        sigvec.push(1); // sighash all

        let mut map = HashMap::new();
        assert!(pt.satisfy(&map, &HashMap::new(), &HashMap::new(), 0).is_err());

        map.insert(keys[0].clone(), (sig.clone(), SigHashType::All));
        map.insert(keys[1].clone(), (sig.clone(), SigHashType::All));
        assert!(pt.satisfy(&map, &HashMap::new(), &HashMap::new(), 0).is_err());

        map.insert(keys[2].clone(), (sig.clone(), SigHashType::All));
        assert_eq!(
            pt.satisfy(&map, &HashMap::new(), &HashMap::new(), 0).unwrap(),
            vec![
                sigvec.clone(),
                sigvec.clone(),
                sigvec.clone(),
                vec![],
            ]
        );

        map.insert(keys[5].clone(), (sig.clone(), SigHashType::All));
        assert_eq!(
            pt.satisfy(&map, &HashMap::new(), &HashMap::new(), 0).unwrap(),
            vec![
                sigvec.clone(),
                sigvec.clone(),
                sigvec.clone(),
                vec![],
            ]
        );

        map.insert(keys[6].clone(), (sig.clone(), SigHashType::All));
        assert_eq!(
            pt.satisfy(&map, &HashMap::new(), &HashMap::new(), 10000).unwrap(),
            vec![
                // sat for right branch
                sigvec.clone(),
                sigvec.clone(),
                vec![],
                // dissat for left branch
                vec![],
                vec![],
                vec![],
                vec![],
            ]
        );
    }

    #[test]
    fn parse_descriptor() {
        assert!(Descriptor::<secp256k1::PublicKey>::from_str("(").is_err());
        assert!(Descriptor::<secp256k1::PublicKey>::from_str("(x()").is_err());
        assert!(Descriptor::<secp256k1::PublicKey>::from_str("(\u{7f}()3").is_err());
        assert!(Descriptor::<secp256k1::PublicKey>::from_str("pk()").is_err());

        assert!(Descriptor::<secp256k1::PublicKey>::from_str("pk(020000000000000000000000000000000000000000000000000000000000000002)").is_ok());
    }
}

