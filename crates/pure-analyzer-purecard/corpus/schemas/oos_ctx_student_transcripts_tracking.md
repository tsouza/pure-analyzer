# Arm C translation context: student_transcripts_tracking

## Execution coordinates

- project_id: `spider-schema-bridge`
- workspace: `student-transcripts-tracking-1783578426`
- database_path: `spider::student_transcripts_tracking::Db`
- autogen_mapping_path (mapping for CLASS-anchored lambda execution): `spider::student_transcripts_tracking::model::DbMapping`
- class_runtime_path (runtime for CLASS-anchored lambda execution): `spider::student_transcripts_tracking::ClassRt`
- empty_mapping_path (store-level tableToTDS lambdas only): `spider::student_transcripts_tracking::EmptyMapping`
- store_runtime_path (store-level tableToTDS lambdas only): `spider::student_transcripts_tracking::Rt`
- classes: `spider::student_transcripts_tracking::model::default::Addresses`, `spider::student_transcripts_tracking::model::default::Courses`, `spider::student_transcripts_tracking::model::default::DegreePrograms`, `spider::student_transcripts_tracking::model::default::Departments`, `spider::student_transcripts_tracking::model::default::Sections`, `spider::student_transcripts_tracking::model::default::Semesters`, `spider::student_transcripts_tracking::model::default::StudentEnrolment`, `spider::student_transcripts_tracking::model::default::StudentEnrolmentCourses`, `spider::student_transcripts_tracking::model::default::Students`, `spider::student_transcripts_tracking::model::default::TranscriptContents`, `spider::student_transcripts_tracking::model::default::Transcripts`
- associations: `spider::student_transcripts_tracking::model::fk_0`, `spider::student_transcripts_tracking::model::fk_1`, `spider::student_transcripts_tracking::model::fk_10`, `spider::student_transcripts_tracking::model::fk_2`, `spider::student_transcripts_tracking::model::fk_3`, `spider::student_transcripts_tracking::model::fk_4`, `spider::student_transcripts_tracking::model::fk_5`, `spider::student_transcripts_tracking::model::fk_6`, `spider::student_transcripts_tracking::model::fk_7`, `spider::student_transcripts_tracking::model::fk_8`, `spider::student_transcripts_tracking::model::fk_9`

## Pure model (autogen classes + associations — what the translator queries)

```pure
Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::student_transcripts_tracking::model::default::Addresses
{
  addressId: Integer[1];
  line1: String[0..1];
  line2: String[0..1];
  line3: String[0..1];
  city: String[0..1];
  zipPostcode: String[0..1];
  stateProvinceCounty: String[0..1];
  country: String[0..1];
  otherAddressDetails: String[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::student_transcripts_tracking::model::default::Courses
{
  courseId: Integer[1];
  courseName: String[0..1];
  courseDescription: String[0..1];
  otherDetails: String[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::student_transcripts_tracking::model::default::DegreePrograms
{
  degreeProgramId: Integer[1];
  departmentId: Integer[0..1];
  degreeSummaryName: String[0..1];
  degreeSummaryDescription: String[0..1];
  otherDetails: String[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::student_transcripts_tracking::model::default::Departments
{
  departmentId: Integer[1];
  departmentName: String[0..1];
  departmentDescription: String[0..1];
  otherDetails: String[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::student_transcripts_tracking::model::default::Sections
{
  sectionId: Integer[1];
  courseId: Integer[0..1];
  sectionName: String[0..1];
  sectionDescription: String[0..1];
  otherDetails: String[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::student_transcripts_tracking::model::default::Semesters
{
  semesterId: Integer[1];
  semesterName: String[0..1];
  semesterDescription: String[0..1];
  otherDetails: String[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::student_transcripts_tracking::model::default::StudentEnrolment
{
  studentEnrolmentId: Integer[1];
  degreeProgramId: Integer[0..1];
  semesterId: Integer[0..1];
  studentId: Integer[0..1];
  otherDetails: String[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::student_transcripts_tracking::model::default::StudentEnrolmentCourses
{
  studentCourseId: Integer[1];
  courseId: Integer[0..1];
  studentEnrolmentId: Integer[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::student_transcripts_tracking::model::default::Students
{
  studentId: Integer[1];
  currentAddressId: Integer[0..1];
  permanentAddressId: Integer[0..1];
  firstName: String[0..1];
  middleName: String[0..1];
  lastName: String[0..1];
  cellMobileNumber: String[0..1];
  emailAddress: String[0..1];
  ssn: String[0..1];
  dateFirstRegistered: DateTime[0..1];
  dateLeft: DateTime[0..1];
  otherStudentDetails: String[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::student_transcripts_tracking::model::default::TranscriptContents
{
  studentCourseId: Integer[0..1];
  transcriptId: Integer[0..1];
}

Class {meta::pure::profiles::doc.doc = 'Generated Element'} spider::student_transcripts_tracking::model::default::Transcripts
{
  transcriptId: Integer[1];
  transcriptDate: DateTime[0..1];
  otherDetails: String[0..1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::student_transcripts_tracking::model::fk_0
{
  fk0DefaultDegreePrograms: spider::student_transcripts_tracking::model::default::DegreePrograms[1..*];
  fk0DefaultDepartments: spider::student_transcripts_tracking::model::default::Departments[1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::student_transcripts_tracking::model::fk_1
{
  fk1DefaultSections: spider::student_transcripts_tracking::model::default::Sections[1..*];
  fk1DefaultCourses: spider::student_transcripts_tracking::model::default::Courses[1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::student_transcripts_tracking::model::fk_10
{
  fk10DefaultTranscriptContents: spider::student_transcripts_tracking::model::default::TranscriptContents[1..*];
  fk10DefaultStudentEnrolmentCourses: spider::student_transcripts_tracking::model::default::StudentEnrolmentCourses[1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::student_transcripts_tracking::model::fk_2
{
  fk2DefaultStudents: spider::student_transcripts_tracking::model::default::Students[1..*];
  fk2DefaultAddresses: spider::student_transcripts_tracking::model::default::Addresses[1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::student_transcripts_tracking::model::fk_3
{
  fk3DefaultStudents: spider::student_transcripts_tracking::model::default::Students[1..*];
  fk3DefaultAddresses: spider::student_transcripts_tracking::model::default::Addresses[1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::student_transcripts_tracking::model::fk_4
{
  fk4DefaultStudentEnrolment: spider::student_transcripts_tracking::model::default::StudentEnrolment[1..*];
  fk4DefaultStudents: spider::student_transcripts_tracking::model::default::Students[1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::student_transcripts_tracking::model::fk_5
{
  fk5DefaultStudentEnrolment: spider::student_transcripts_tracking::model::default::StudentEnrolment[1..*];
  fk5DefaultSemesters: spider::student_transcripts_tracking::model::default::Semesters[1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::student_transcripts_tracking::model::fk_6
{
  fk6DefaultStudentEnrolment: spider::student_transcripts_tracking::model::default::StudentEnrolment[1..*];
  fk6DefaultDegreePrograms: spider::student_transcripts_tracking::model::default::DegreePrograms[1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::student_transcripts_tracking::model::fk_7
{
  fk7DefaultStudentEnrolmentCourses: spider::student_transcripts_tracking::model::default::StudentEnrolmentCourses[1..*];
  fk7DefaultStudentEnrolment: spider::student_transcripts_tracking::model::default::StudentEnrolment[1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::student_transcripts_tracking::model::fk_8
{
  fk8DefaultStudentEnrolmentCourses: spider::student_transcripts_tracking::model::default::StudentEnrolmentCourses[1..*];
  fk8DefaultCourses: spider::student_transcripts_tracking::model::default::Courses[1];
}

Association {meta::pure::profiles::doc.doc = 'Generated Element'} spider::student_transcripts_tracking::model::fk_9
{
  fk9DefaultTranscriptContents: spider::student_transcripts_tracking::model::default::TranscriptContents[1..*];
  fk9DefaultTranscripts: spider::student_transcripts_tracking::model::default::Transcripts[1];
}
```

## Glossary (question vocabulary => model identifiers)

```
Glossary for database 'student_transcripts_tracking'.
Maps question vocabulary (human-readable names) to model identifiers.

Table 'addresses' => Addresses
  - column 'address id' => Addresses.address_id
  - column 'line 1' => Addresses.line_1
  - column 'line 2' => Addresses.line_2
  - column 'line 3' => Addresses.line_3
  - column 'city' => Addresses.city
  - column 'zip postcode' => Addresses.zip_postcode
  - column 'state province county' => Addresses.state_province_county
  - column 'country' => Addresses.country
  - column 'other address details' => Addresses.other_address_details
Table 'courses' => Courses
  - column 'course id' => Courses.course_id
  - column 'course name' => Courses.course_name
  - column 'course description' => Courses.course_description
  - column 'other details' => Courses.other_details
Table 'departments' => Departments
  - column 'department id' => Departments.department_id
  - column 'department name' => Departments.department_name
  - column 'department description' => Departments.department_description
  - column 'other details' => Departments.other_details
Table 'degree programs' => Degree_Programs
  - column 'degree program id' => Degree_Programs.degree_program_id
  - column 'department id' => Degree_Programs.department_id
  - column 'degree summary name' => Degree_Programs.degree_summary_name
  - column 'degree summary description' => Degree_Programs.degree_summary_description
  - column 'other details' => Degree_Programs.other_details
Table 'sections' => Sections
  - column 'section id' => Sections.section_id
  - column 'course id' => Sections.course_id
  - column 'section name' => Sections.section_name
  - column 'section description' => Sections.section_description
  - column 'other details' => Sections.other_details
Table 'semesters' => Semesters
  - column 'semester id' => Semesters.semester_id
  - column 'semester name' => Semesters.semester_name
  - column 'semester description' => Semesters.semester_description
  - column 'other details' => Semesters.other_details
Table 'students' => Students
  - column 'student id' => Students.student_id
  - column 'current address id' => Students.current_address_id
  - column 'permanent address id' => Students.permanent_address_id
  - column 'first name' => Students.first_name
  - column 'middle name' => Students.middle_name
  - column 'last name' => Students.last_name
  - column 'cell mobile number' => Students.cell_mobile_number
  - column 'email address' => Students.email_address
  - column 'ssn' => Students.ssn
  - column 'date first registered' => Students.date_first_registered
  - column 'date left' => Students.date_left
  - column 'other student details' => Students.other_student_details
Table 'student enrolment' => Student_Enrolment
  - column 'student enrolment id' => Student_Enrolment.student_enrolment_id
  - column 'degree program id' => Student_Enrolment.degree_program_id
  - column 'semester id' => Student_Enrolment.semester_id
  - column 'student id' => Student_Enrolment.student_id
  - column 'other details' => Student_Enrolment.other_details
Table 'student enrolment courses' => Student_Enrolment_Courses
  - column 'student course id' => Student_Enrolment_Courses.student_course_id
  - column 'course id' => Student_Enrolment_Courses.course_id
  - column 'student enrolment id' => Student_Enrolment_Courses.student_enrolment_id
Table 'transcripts' => Transcripts
  - column 'transcript id' => Transcripts.transcript_id
  - column 'transcript date' => Transcripts.transcript_date
  - column 'other details' => Transcripts.other_details
Table 'transcript contents' => Transcript_Contents
  - column 'student course id' => Transcript_Contents.student_course_id
  - column 'transcript id' => Transcript_Contents.transcript_id
```

## PMCD for compile checks (fetch/assemble)

Two equivalent routes to the PureModelContextData a candidate lambda is compiled against:

1. **Fetch from the workspace (canonical for this probe):**
   `GET http://localhost:6100/api/projects/spider-schema-bridge/workspaces/student-transcripts-tracking-1783578426/pureModelContextData`
   then drop every element with `_type == "sectionIndex"`. VERIFY presence of all required
   paths (database, DbMapping, ClassRt, Rt, EmptyMapping, Conn, classes, associations) --
   fs-SDLC silently drops elements its bundled protocol can't deserialize (gate0-findings).

2. **Assemble from grammar (what run_pilot does per instance):** concatenate the store
   grammar + connection grammar + autogen class/association/mapping grammar and parse via
   `POST http://localhost:6300/api/pure/v1/grammar/grammarToJson` (see `assemble_pmcd` in
   `src/pure_lingua/schema_bridge/model_bootstrap.py`; cached by grammar hash).

**Compile check:** `POST http://localhost:6300/api/pure/v1/compilation/lambdaReturnType` with body exactly
`{"lambda": <lambda json>, "model": <pmcd>}` -- no `clientVersion` field (the endpoint
rejects it outright). Success = HTTP 200 with a `returnType`; anything else is a compile
failure. Class-anchored lambdas execute with mapping `spider::student_transcripts_tracking::model::DbMapping` + runtime `spider::student_transcripts_tracking::ClassRt`.
