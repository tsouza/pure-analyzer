use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawClass {
    pub(crate) package: String,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) super_types: Vec<RawPath>,
    #[serde(default)]
    pub(crate) stereotypes: Vec<RawStereotype>,
    #[serde(default)]
    pub(crate) properties: Vec<RawProperty>,
    #[serde(default)]
    pub(crate) qualified_properties: Vec<RawQualifiedProperty>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RawAssociation {
    pub(crate) package: String,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) stereotypes: Vec<RawStereotype>,
    pub(crate) properties: Vec<RawProperty>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum RawPath {
    String(String),
    Pointer { path: String },
}

impl RawPath {
    pub(crate) fn into_string(self) -> String {
        match self {
            Self::String(path) | Self::Pointer { path } => path,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct RawStereotype {
    pub(crate) profile: String,
    pub(crate) value: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawProperty {
    pub(crate) name: String,
    pub(crate) generic_type: RawGenericType,
    pub(crate) multiplicity: RawMultiplicity,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawQualifiedProperty {
    pub(crate) name: String,
    #[serde(alias = "genericType")]
    pub(crate) return_generic_type: RawGenericType,
    #[serde(alias = "multiplicity")]
    pub(crate) return_multiplicity: RawMultiplicity,
    #[serde(default)]
    pub(crate) stereotypes: Vec<RawStereotype>,
    pub(crate) parameters: Option<Vec<RawParameter>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawParameter {
    pub(crate) generic_type: RawGenericType,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawGenericType {
    pub(crate) raw_type: RawTypeName,
    #[serde(default)]
    pub(crate) type_arguments: Vec<RawGenericType>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum RawTypeName {
    String(String),
    Object {
        #[serde(rename = "fullPath")]
        full_path: String,
    },
}

impl RawTypeName {
    pub(crate) fn into_string(self) -> String {
        match self {
            Self::String(path) | Self::Object { full_path: path } => path,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawMultiplicity {
    pub(crate) lower_bound: u32,
    #[serde(default)]
    pub(crate) upper_bound: Option<u32>,
}
