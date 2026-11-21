//! Parsing RFEM's `get_calculation_errors` reply. On the owner's PC antivirus blocks this
//! crate's test exes ("Access is denied"); CI runs them.

use femcraft_rfem::calculation_messages;

#[test]
fn calculation_errors_reply() {
    let errs = "<s:Body><get_calculation_errorsResponse><value>\
        <message><message_type>ERROR</message_type><object>Member 3</object>\
        <input_field>Section</input_field><message>No section &amp; no material</message>\
        <result>false</result></message>\
        <message><message_type>WARNING</message_type><message>Unstable</message></message>\
        </value></get_calculation_errorsResponse></s:Body>";
    assert_eq!(
        calculation_messages(errs),
        [
            "ERROR: No section & no material (object Member 3, field Section)",
            "WARNING: Unstable"
        ]
    );
    let ns = "<ns:value><ns:item><ns:message_type>ERROR</ns:message_type>\
        <ns:message>Bad</ns:message></ns:item></ns:value>";
    assert_eq!(calculation_messages(ns), ["ERROR: Bad"]);
}
