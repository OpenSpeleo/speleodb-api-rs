use serde_json::json;
use speleodb_api::types::{
    CylinderPressureCheckRequest, LandmarkFeatureProperties, PatchedCylinderPressureCheckRequest,
    V2CylindersPartialUpdateWithJsonRequest,
};

#[test]
fn calendar_dates_preserve_json_and_form_encoding_and_patch_omission() {
    let wire = json!({"check_date": "2024-02-29", "pressure": 200, "unit_system": "metric"});
    let request: CylinderPressureCheckRequest = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(request.check_date.day(), 29);
    assert_eq!(serde_json::to_value(&request).unwrap(), wire);
    let form = serde_urlencoded::to_string(&request).unwrap();
    assert!(
        form.split('&')
            .any(|field| field == "check_date=2024-02-29")
    );

    for date in [
        "2023-02-29",
        "2024-02-30",
        "29/02/2024",
        "2024-02-29T00:00:00Z",
    ] {
        let mut invalid = wire.clone();
        invalid["check_date"] = json!(date);
        assert!(serde_json::from_value::<CylinderPressureCheckRequest>(invalid).is_err());
    }

    for wire in [json!({}), json!({"check_date": "2024-02-29"})] {
        let patch: PatchedCylinderPressureCheckRequest =
            serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(&patch).unwrap(), wire);
        let form = serde_urlencoded::to_string(&patch).unwrap();
        assert_eq!(
            form.contains("check_date="),
            wire.get("check_date").is_some()
        );
    }
}

#[test]
fn nullable_calendar_date_patch_distinguishes_omission_clear_and_replacement() {
    for wire in [
        json!({}),
        json!({"manufactured_date": null}),
        json!({"manufactured_date": "2024-02-29"}),
    ] {
        let request: V2CylindersPartialUpdateWithJsonRequest =
            serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(request).unwrap(), wire);
    }
    assert!(
        serde_json::from_value::<V2CylindersPartialUpdateWithJsonRequest>(
            json!({"manufactured_date": "2023-02-29"})
        )
        .is_err()
    );
}

#[test]
fn nullable_landmark_timestamp_preserves_null_offsets_and_nanoseconds() {
    let mut wire = json!({
        "can_delete": false, "can_write": false,
        "collection": uuid::Uuid::nil(), "collection_color": "#94a3b8",
        "collection_name": "Survey", "collection_type": "personal",
        "created_by": "Surveyor", "description": "Entrance", "icon": "CAVE",
        "is_personal_collection": true, "name": "Entrance"
    });
    for timestamp in [
        json!(null),
        json!("2026-11-10T03:18:23.123456789-05:00"),
        json!("2026-11-10T08:18:23Z"),
    ] {
        wire["creation_date"] = timestamp.clone();
        let landmark: LandmarkFeatureProperties = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(landmark).unwrap()["creation_date"],
            timestamp
        );
    }
    for invalid in ["2026-11-10T08:18:23", "2026-02-30T08:18:23Z"] {
        wire["creation_date"] = json!(invalid);
        assert!(serde_json::from_value::<LandmarkFeatureProperties>(wire.clone()).is_err());
    }
}
