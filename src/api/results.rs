use std::fmt::Debug;

use bytes::BytesMut;
use futures::{
    stream::{BoxStream, StreamExt},
    Stream,
};
use postgres_types::{IsNull, ToSql, Type};

use crate::{
    error::{ErrorInfo, PgWireResult},
    messages::{
        data::{DataRow, FieldDescription, RowDescription, FORMAT_CODE_BINARY, FORMAT_CODE_TEXT},
        response::CommandComplete,
    },
    types::ToSqlText,
};

#[derive(Debug, Eq, PartialEq)]
pub struct Tag {
    command: String,
    rows: Option<usize>,
}

impl Tag {
    pub fn new_for_query(rows: usize) -> Tag {
        Tag {
            command: "SELECT".to_owned(),
            rows: Some(rows),
        }
    }

    pub fn new_for_execution(command: &str, rows: Option<usize>) -> Tag {
        Tag {
            command: command.to_owned(),
            rows,
        }
    }
}

impl From<Tag> for CommandComplete {
    fn from(tag: Tag) -> CommandComplete {
        let tag_string = if let Some(rows) = tag.rows {
            format!("{} {rows}", tag.command)
        } else {
            tag.command
        };
        CommandComplete::new(tag_string)
    }
}

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub enum FieldFormat {
    Text,
    Binary,
}

impl FieldFormat {
    pub(crate) fn value(&self) -> i16 {
        match self {
            Self::Text => FORMAT_CODE_TEXT,
            Self::Binary => FORMAT_CODE_BINARY,
        }
    }
}

#[derive(Debug, new, Eq, PartialEq, Clone, Getters)]
#[getset(get = "pub")]
pub struct FieldInfo {
    name: String,
    table_id: Option<i32>,
    column_id: Option<i16>,
    datatype: Type,
    format: FieldFormat,
}

impl From<FieldInfo> for FieldDescription {
    fn from(fi: FieldInfo) -> Self {
        FieldDescription::new(
            fi.name,                   // name
            fi.table_id.unwrap_or(0),  // table_id
            fi.column_id.unwrap_or(0), // column_id
            fi.datatype.oid(),         // type_id
            // TODO: type size and modifier
            0,
            0,
            fi.format.value(),
        )
    }
}

pub(crate) fn into_row_description(fields: Vec<FieldInfo>) -> RowDescription {
    RowDescription::new(fields.into_iter().map(Into::into).collect())
}

#[derive(Getters)]
#[getset(get = "pub")]
pub struct QueryResponse<'a> {
    pub(crate) row_schema: Option<Vec<FieldInfo>>,
    pub(crate) data_rows: BoxStream<'a, PgWireResult<DataRow>>,
}

pub struct DataRowEncoder {
    buffer: DataRow,
    field_buffer: BytesMut,
}

impl DataRowEncoder {
    pub fn new(ncols: usize) -> DataRowEncoder {
        Self {
            buffer: DataRow::new(Vec::with_capacity(ncols)),
            field_buffer: BytesMut::with_capacity(8),
        }
    }

    pub fn encode_field<T>(&mut self, value: &T, data_type: &Type, format: i16) -> PgWireResult<()>
    where
        T: ToSql + ToSqlText + Sized,
    {
        let is_null = if format == FORMAT_CODE_TEXT {
            value.to_sql_text(data_type, &mut self.field_buffer)?
        } else {
            value.to_sql(data_type, &mut self.field_buffer)?
        };

        if let IsNull::No = is_null {
            let buf = self.field_buffer.split().freeze();
            self.buffer.fields_mut().push(Some(buf));
        } else {
            self.buffer.fields_mut().push(None);
        }
        self.field_buffer.clear();
        Ok(())
    }

    pub fn finish(self) -> PgWireResult<DataRow> {
        Ok(self.buffer)
    }
}

pub fn query_response<'a, S>(field_defs: Option<Vec<FieldInfo>>, row_stream: S) -> QueryResponse<'a>
where
    S: Stream<Item = PgWireResult<DataRow>> + Send + Unpin + 'a,
{
    QueryResponse {
        row_schema: field_defs,
        data_rows: row_stream.boxed(),
    }
}

/// Response for frontend describe requests.
///
/// There are two types of describe: statement and portal. When describing
/// statement, frontend expects parameter types inferenced by server. And both
/// describe messages will require column definitions for resultset being
/// returned.
#[derive(Debug, Getters, new)]
#[getset(get = "pub")]
pub struct DescribeResponse {
    parameters: Option<Vec<Type>>,
    fields: Vec<FieldInfo>,
}

impl DescribeResponse {
    pub(crate) fn take_fields(self) -> Vec<FieldInfo> {
        self.fields
    }
}

/// Query response types:
///
/// * Query: the response contains data rows
/// * Execution: response for ddl/dml execution
/// * Error: error response
pub enum Response<'a> {
    Query(QueryResponse<'a>),
    Execution(Tag),
    Error(Box<ErrorInfo>),
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_command_complete() {
        let tag = Tag::new_for_execution("INSERT", Some(100));
        let cc = CommandComplete::from(tag);

        assert_eq!(cc.tag(), "INSERT 100");
    }
}
