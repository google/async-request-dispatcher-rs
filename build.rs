// Copyright 2025 Google LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

fn main() -> std::io::Result<()> {
    let protos = vec!["googleapis/google/cloud/aiplatform/v1/*.proto"]
        .into_iter()
        .map(|f| {
            glob::glob(f)
                .unwrap()
                .filter_map(|r| r.ok())
                .collect::<Vec<_>>()
        })
        .flatten()
        .collect::<Vec<_>>();

    tonic_prost_build::configure()
        .type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]")
        .extern_path(".google.protobuf.Timestamp", "::prost_wkt_types::Timestamp")
        .extern_path(".google.protobuf.Struct", "::prost_wkt_types::Struct")
        .extern_path(".google.protobuf.Duration", "::prost_wkt_types::Duration")
        .extern_path(".google.protobuf.Any", "::prost_wkt_types::Any")
        .extern_path(".google.protobuf.Value", "::prost_wkt_types::Value")
        .extern_path(".google.protobuf.ListValue", "::prost_wkt_types::ListValue")
        .extern_path(".google.protobuf.FieldMask", "::prost_wkt_types::FieldMask")
        .include_file("gcp.rs")
        .compile_protos(&protos, &["googleapis".into()])?;

    Ok(())
}
