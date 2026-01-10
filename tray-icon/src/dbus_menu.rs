// Currently borrowed from: https://github.com/iovxw/ksni/blob/master/src/dbus_interface.rs
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use zbus::Connection;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedValue;
use zbus::zvariant::Type;
use zbus::zvariant::Value;

#[derive(Debug, Default, Type, Serialize, Deserialize, Value, OwnedValue)]
pub struct Layout {
    pub id: i32,
    pub properties: HashMap<String, OwnedValue>,
    pub children: Vec<OwnedValue>,
}

pub struct DbusMenu();

impl DbusMenu {
    pub fn new() -> Self {
        DbusMenu()
    }
}

#[zbus::interface(name = "com.canonical.dbusmenu")]
impl DbusMenu {
    // methods
    async fn get_layout(
        &self,
        parent_id: i32,
        _recursion_depth: i32,
        _property_names: Vec<String>,
    ) -> zbus::fdo::Result<(u32, Layout)> {
        println!("get_layout called for parent_id {}", parent_id);
        if parent_id == 0 {
            let mut quit_properties = HashMap::new();
            quit_properties.insert(
                "label".to_string(),
                OwnedValue::try_from(Value::new("Quit")).unwrap(),
            );

            let quit_child = Layout {
                id: 1,
                properties: quit_properties,
                children: vec![],
            };

            Ok((
                0,
                Layout {
                    id: parent_id,
                    properties: HashMap::new(),
                    children: vec![OwnedValue::try_from(quit_child).unwrap()],
                },
            ))
        } else {
            Err(zbus::fdo::Error::InvalidArgs(
                "parentId not found".to_string(),
            ))
        }
    }

    async fn get_group_properties(
        &self,
        _ids: Vec<i32>,
        _property_names: Vec<String>,
    ) -> zbus::fdo::Result<Vec<(i32, HashMap<String, OwnedValue>)>> {
        Ok(Vec::new())
    }

    async fn get_property(&self, id: i32, name: String) -> zbus::fdo::Result<OwnedValue> {
        Err(zbus::fdo::Error::InvalidArgs(format!(
            "Property '{}' for id {} not found",
            name, id
        )))
    }

    async fn event(
        &self,
        #[zbus(connection)] _conn: &Connection,
        _id: i32,
        _event_id: String,
        _data: OwnedValue,
        _timestamp: u32,
    ) -> zbus::fdo::Result<()> {
        println!(
            "Event received for id {} {} {} {}",
            _id,
            _event_id,
            _timestamp,
            _data.to_string()
        );
        Ok(())
    }

    async fn event_group(
        &self,
        #[zbus(connection)] _conn: &Connection,
        _events: Vec<(i32, String, OwnedValue, u32)>,
    ) -> zbus::fdo::Result<Vec<i32>> {
        Ok(vec![])
    }

    async fn about_to_show(&self) -> zbus::fdo::Result<bool> {
        Ok(false)
    }

    async fn about_to_show_group(&self) -> zbus::fdo::Result<(Vec<i32>, Vec<i32>)> {
        Ok(Default::default())
    }

    // properties
    #[zbus(property)]
    fn version(&self) -> zbus::fdo::Result<u32> {
        Ok(3)
    }

    #[zbus(property)]
    async fn text_direction(&self) -> zbus::fdo::Result<String> {
        Ok("ltr".to_string())
    }

    #[zbus(property)]
    async fn status(&self) -> zbus::fdo::Result<String> {
        Ok("normal".to_string())
    }

    #[zbus(property)]
    async fn icon_theme_path(&self) -> zbus::fdo::Result<Vec<String>> {
        Ok(vec![])
    }

    // signals
    #[zbus(signal)]
    pub async fn items_properties_updated(
        ctxt: &SignalEmitter<'_>,
        updated_props: Vec<(i32, HashMap<String, OwnedValue>)>,
        removed_props: Vec<(i32, Vec<String>)>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn layout_updated(
        ctxt: &SignalEmitter<'_>,
        revision: u32,
        parent: i32,
    ) -> zbus::Result<()>;
}
