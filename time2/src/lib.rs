pub const RFC3339_MILLISECONDS: &[time::format_description::FormatItem<'_>] = &[
	time::format_description::FormatItem::Component(time::format_description::Component::Year(time::format_description::modifier::Year::default())),
	time::format_description::FormatItem::Literal(b"-"),
	time::format_description::FormatItem::Component(time::format_description::Component::Month(time::format_description::modifier::Month::default())),
	time::format_description::FormatItem::Literal(b"-"),
	time::format_description::FormatItem::Component(time::format_description::Component::Day(time::format_description::modifier::Day::default())),
	time::format_description::FormatItem::Literal(b"T"),
	time::format_description::FormatItem::Component(time::format_description::Component::Hour(time::format_description::modifier::Hour::default())),
	time::format_description::FormatItem::Literal(b":"),
	time::format_description::FormatItem::Component(time::format_description::Component::Minute(time::format_description::modifier::Minute::default())),
	time::format_description::FormatItem::Literal(b":"),
	time::format_description::FormatItem::Component(time::format_description::Component::Second(time::format_description::modifier::Second::default())),
	time::format_description::FormatItem::Literal(b"."),
	time::format_description::FormatItem::Component(time::format_description::Component::Subsecond({
		let mut subsecond = time::format_description::modifier::Subsecond::default();
		subsecond.digits = time::format_description::modifier::SubsecondDigits::Three;
		subsecond
	})),
	time::format_description::FormatItem::Literal(b"Z"),
];
