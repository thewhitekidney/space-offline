//! `corpRecProxy`: the corporation recruitment adverts, **empty** (S1 of
//! `soc-answers`, #158).
//!
//! No advert exists here yet, but the Recruitment tab and the corp finder
//! iterate what these calls return (`bco_recruitment.
//! GetRecruitmentAdsForCorporation`, `corpRecruitmentContainer`'s search and
//! by-corporation paths), and an unanswered call is `None`, which is a Python
//! error in the corp window. A list is the true answer for "none": the
//! finder's `SearchCorpAds` reads `(grade, advert)` pairs, so an empty list
//! has no pair to get wrong. A later lane (S8) fills these in.

use super::*;

/// The services dispatched here. `Registry::dispatch` consults this to route.
pub(crate) const SERVICES: &[&str] = &["corpRecProxy"];

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.method {
        "GetRecruitmentAdsForCorporation" | "GetRecruitmentAdsByCorpID" | "SearchCorpAds" => {
            Ok(PyValue::List(vec![]))
        }
        _ => Err(reg.no_method(req)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_finder_answers_empty_lists() {
        let mut reg = Registry::new(crate::model::SandboxConfig::default());
        for method in [
            "GetRecruitmentAdsForCorporation",
            "GetRecruitmentAdsByCorpID",
            "SearchCorpAds",
        ] {
            let req = Request {
                service: "corpRecProxy",
                method,
                args: &[],
                kwargs: &[],
                bind_params: None,
                actor: None,
            };
            assert_eq!(
                dispatch(&mut reg, &req).unwrap(),
                PyValue::List(vec![]),
                "{method}"
            );
        }
    }
}
