//! Independent wire and hostile-input checks for read-only SDK conversion.
use super::*;

#[test]
fn controller_conversion_preserves_text_identity_and_refuses_fabricated_values() {
    let config = config();
    let query = ParameterPageQuery {
        conversion: Some(ParameterConversion::Parse {
            id: 0,
            text: ParameterText::from_text("double 𝄞").unwrap(),
        }),
        revision: 9,
        ..ParameterPageQuery::default()
    };
    let request = parameter_page_request(&config, 1, 7, query, false).unwrap();
    assert_eq!(request.len(), 351);
    assert_eq!(request[82], 2);
    assert_eq!(&request[83..95], &[0; 12]);
    let expected: Vec<_> = "double 𝄞"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    assert_eq!(&request[95..95 + expected.len()], expected);
    assert!(request[95 + expected.len()..].iter().all(|byte| *byte == 0));
    assert!(ParameterText::from_text("a\0b").is_err());
    assert!(ParameterText::from_text(&"a".repeat(127)).is_ok());
    assert!(ParameterText::from_text(&"a".repeat(128)).is_err());
    assert!(ParameterText::from_text(&"𝄞".repeat(63)).is_ok());
    assert!(ParameterText::from_text(&"𝄞".repeat(64)).is_err());

    let mut body = Vec::new();
    stamp(&mut body, config.binding);
    for value in [1_u64, 8, 7] {
        body.extend(value.to_le_bytes());
    }
    body.push(3);
    body.extend(9_u64.to_le_bytes());
    body.extend(1_u16.to_le_bytes());
    body.extend(0_u32.to_le_bytes());
    body.push(0);
    body.extend(0.5_f64.to_le_bytes());
    body.push(0);
    body.extend(2.0_f64.to_le_bytes());
    body.push(1);
    let text = ParameterText::from_text("double 𝄞").unwrap();
    let mut units = [0_u16; 128];
    units[..text.as_utf16().len()].copy_from_slice(text.as_utf16());
    for unit in units {
        body.extend(unit.to_le_bytes());
    }
    let ParameterPageProgress::Conversion(value) =
        parameter_page(&body, &config, 1, 7, query).unwrap()
    else {
        panic!("actual conversion response");
    };
    assert_eq!(
        (value.revision, value.total, value.id, value.normalized),
        (9, 1, 0, Some(0.5))
    );
    assert_eq!(value.status, ParameterConversionStatus::Converted);
    assert_eq!(value.plain, ParameterPlainValue::Finite(2.0));
    assert_eq!(value.display.unwrap(), text);
    for end in 0..body.len() {
        assert!(
            parameter_page(&body[..end], &config, 1, 7, query).is_err(),
            "truncation {end}"
        );
    }
    for (offset, bytes) in [
        (58, vec![1]),
        (67, 0_u16.to_le_bytes().to_vec()),
        (69, 3_u32.to_le_bytes().to_vec()),
        (73, vec![4]),
        (74, f64::NAN.to_le_bytes().to_vec()),
        (74, 1.01_f64.to_le_bytes().to_vec()),
        (82, vec![4]),
        (82, vec![1]),
        (91, vec![2]),
        (92, 0_u16.to_le_bytes().to_vec()),
        (92, 0xD800_u16.to_le_bytes().to_vec()),
    ] {
        let mut bad = body.clone();
        bad[offset..offset + bytes.len()].copy_from_slice(&bytes);
        assert!(
            parameter_page(&bad, &config, 1, 7, query).is_err(),
            "invalid field at {offset}"
        );
    }
    let mut bad = body.clone();
    bad.push(0);
    assert!(parameter_page(&bad, &config, 1, 7, query).is_err());
    let format = ParameterPageQuery {
        conversion: Some(ParameterConversion::Format {
            id: 0,
            normalized: 0.25,
        }),
        ..query
    };
    assert!(
        parameter_page(&body, &config, 1, 7, format).is_err(),
        "format must echo the requested normalized value"
    );
    for status in [1, 2, 3] {
        let mut refused = body.clone();
        refused[73] = status;
        refused[74..].fill(0);
        refused[82] = 3;
        let ParameterPageProgress::Conversion(value) =
            parameter_page(&refused, &config, 1, 7, query).unwrap()
        else {
            panic!("refused conversion");
        };
        assert_eq!(value.normalized, None);
        assert_eq!(value.plain, ParameterPlainValue::Unavailable);
        assert_eq!(value.display, None);
        refused[74..82].copy_from_slice(&0.5_f64.to_le_bytes());
        assert!(
            parameter_page(&refused, &config, 1, 7, query).is_err(),
            "refusal cannot return a value"
        );
    }
    for conversion in [
        ParameterConversion::Format {
            id: 0,
            normalized: f64::NAN,
        },
        ParameterConversion::Format {
            id: 0,
            normalized: 1.1,
        },
        ParameterConversion::Format {
            id: u32::MAX,
            normalized: 0.5,
        },
        ParameterConversion::FromPlain {
            id: 0,
            plain: ParameterPlainValue::Unavailable,
        },
        ParameterConversion::FromPlain {
            id: 0,
            plain: ParameterPlainValue::Finite(f64::INFINITY),
        },
    ] {
        assert!(
            parameter_page_request(
                &config,
                1,
                7,
                ParameterPageQuery {
                    conversion: Some(conversion),
                    ..query
                },
                false
            )
            .is_err()
        );
    }
    for (plain, kind) in [
        (ParameterPlainValue::NegativeInfinity, 4),
        (ParameterPlainValue::PositiveInfinity, 5),
    ] {
        let bytes = parameter_page_request(
            &config,
            1,
            7,
            ParameterPageQuery {
                conversion: Some(ParameterConversion::FromPlain { id: 0, plain }),
                ..query
            },
            false,
        )
        .unwrap();
        assert_eq!(bytes[82], kind);
        assert!(bytes[83..].iter().all(|byte| *byte == 0));
    }
}
