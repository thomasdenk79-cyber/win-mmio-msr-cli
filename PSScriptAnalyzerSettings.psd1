@{
    Severity     = @('Error', 'Warning', 'Information')
    ExcludeRules = @(
        # Console tools and CI steps report progress to the operator on purpose.
        'PSAvoidUsingWriteHost',
        # Script-internal helpers; every script is an explicit, documented operation.
        'PSUseShouldProcessForStateChangingFunctions'
    )
}
