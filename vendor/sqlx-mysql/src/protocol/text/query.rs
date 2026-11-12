use crate::io::ProtocolEncode;
use crate::protocol::Capabilities;

// https://dev.mysql.com/doc/internals/en/com-query.html

#[derive(Debug)]
pub(crate) struct Query<'q>(pub(crate) &'q str);

impl ProtocolEncode<'_, Capabilities> for Query<'_> {
    fn encode_with(&self, buf: &mut Vec<u8>, _: Capabilities) -> Result<(), crate::Error> {
        buf.push(0x03); // COM_QUERY
        buf.extend(self.0.as_bytes());
        Ok(())
    }
}

/// `COM_INIT_DB` selects the connection's current database. Unlike a `USE` statement it never
/// goes through the SQL parser, which is what MySQL-protocol middlewares (and the official
/// MariaDB Connector/C `mysql_select_db`, used by Navicat) rely on.
#[derive(Debug)]
pub(crate) struct InitDb<'q>(pub(crate) &'q str);

impl ProtocolEncode<'_, Capabilities> for InitDb<'_> {
    fn encode_with(&self, buf: &mut Vec<u8>, _: Capabilities) -> Result<(), crate::Error> {
        buf.push(0x02); // COM_INIT_DB
        buf.extend(self.0.as_bytes());
        Ok(())
    }
}
